use base64::{engine::general_purpose::STANDARD, Engine as _};
use tauri::State;

use crate::agent;
use crate::bundle::{self, ImportSummary};
use crate::lineage::{LineageEngine, Transition};
use crate::runner;
use crate::state::AppState;
use crate::store::StoreError;
#[cfg(test)]
use crate::types::AgentKind;
use crate::types::{
    current_time_unix_ms, AgentAvailability, AppHealth, DelegationSummary, EditableFeedback,
    EditableFeedbackScreen, EntityIdPayload, FeedbackRecord, FeedbackRevision,
    FeedbackScreenRecord, FeedbackStatus, LineageJobRecord, LineageJobStatus,
    LocalDelegateAvailability, PreviewSummary, SubmitFeedbackPayload,
};

const APP_NAME: &str = "Evolvo";
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Per-attachment decoded-byte ceiling. Kept in sync with
/// `store::MAX_ATTACHMENT_BYTES` (the store enforces its own copy as
/// defense in depth); this constant lets `submit_feedback` reject an
/// oversized attachment before spending the cost of base64-decoding it.
const MAX_ATTACHMENT_BYTES: usize = crate::store::MAX_ATTACHMENT_BYTES;
/// Aggregate decoded-byte ceiling across every attachment in a single
/// `SubmitFeedbackPayload` (screenshot + pasted images + voice). Bounds the
/// total IPC payload / disk write a single submission can cost regardless
/// of how the bytes are split across attachments.
const MAX_TOTAL_ATTACHMENT_BYTES: usize = 32 * 1024 * 1024;

fn store_error(err: StoreError) -> String {
    err.to_string()
}

/// Estimate the decoded byte length of a base64 string without decoding it,
/// so oversized payloads can be rejected before paying the decode cost.
/// Standard base64 (no line breaks) encodes every 4 input chars as 3 output
/// bytes, minus 1 or 2 for `=` padding.
fn estimated_decoded_len(b64: &str) -> usize {
    let len = b64.len();
    let padding = b64.chars().rev().take_while(|&c| c == '=').count();
    (len / 4) * 3 - padding.min(len / 4 * 3)
}

/// Reject a base64 attachment before decoding if its estimated decoded size
/// exceeds `MAX_ATTACHMENT_BYTES`. Cheap, and avoids allocating a
/// multi-megabyte `Vec<u8>` for a payload we're going to reject anyway.
fn check_encoded_size(name: &str, b64: &str) -> Result<(), String> {
    let estimated = estimated_decoded_len(b64);
    if estimated > MAX_ATTACHMENT_BYTES {
        return Err(format!(
            "attachment {name} is approximately {estimated} bytes, which exceeds the {MAX_ATTACHMENT_BYTES}-byte per-attachment limit"
        ));
    }
    Ok(())
}

fn decode_base64(name: &str, value: &str) -> Result<Vec<u8>, String> {
    check_encoded_size(name, value)?;
    let bytes = STANDARD
        .decode(value)
        .map_err(|e| format!("invalid base64 for {name}: {e}"))?;
    // Double-check post-decode: the pre-decode estimate is a close but not
    // exact bound (base64 implementations can differ at the margins), and
    // this is the number that actually lands on disk.
    if bytes.len() > MAX_ATTACHMENT_BYTES {
        return Err(format!(
            "attachment {name} is {} bytes, which exceeds the {MAX_ATTACHMENT_BYTES}-byte per-attachment limit",
            bytes.len()
        ));
    }
    Ok(bytes)
}

/// Track and enforce the aggregate decoded-byte budget across every
/// attachment in one `submit_feedback` call. Call `add` after each
/// successful `decode_base64` so a submission with many attachments, each
/// individually under the per-attachment cap, still can't blow past a
/// sane total.
struct AttachmentBudget {
    used: usize,
}

impl AttachmentBudget {
    fn new() -> Self {
        Self { used: 0 }
    }

    fn add(&mut self, name: &str, len: usize) -> Result<(), String> {
        self.used = self.used.saturating_add(len);
        if self.used > MAX_TOTAL_ATTACHMENT_BYTES {
            return Err(format!(
                "attachment {name} pushed total attachment size to {} bytes, which exceeds the {MAX_TOTAL_ATTACHMENT_BYTES}-byte aggregate limit",
                self.used
            ));
        }
        Ok(())
    }
}

fn guess_voice_ext(mime: Option<&str>) -> &'static str {
    match mime.unwrap_or("") {
        "audio/webm" | "audio/webm;codecs=opus" => "webm",
        "audio/ogg" | "audio/ogg;codecs=opus" => "ogg",
        "audio/mp4" | "audio/mpeg" => "m4a",
        "audio/wav" | "audio/x-wav" => "wav",
        _ => "bin",
    }
}

#[tauri::command]
pub fn app_health(state: State<'_, AppState>) -> Result<AppHealth, String> {
    let store = state.store();
    store.init_workspace().map_err(store_error)?;
    Ok(AppHealth {
        app_name: APP_NAME.into(),
        app_version: APP_VERSION.into(),
        workspace_path: state.workspace_root_display(),
        launched_at_unix_ms: state.launched_at_unix_ms,
        active_agent_sessions: crate::concurrency::active_count(),
        max_concurrent_agent_sessions: crate::concurrency::cap(),
        corrupted_entity_count: store.count_quarantined_entities(),
    })
}

#[tauri::command]
pub fn submit_feedback(
    state: State<'_, AppState>,
    payload: SubmitFeedbackPayload,
) -> Result<FeedbackRecord, String> {
    let store = state.store();
    store.init_workspace().map_err(store_error)?;

    if let Some(cfg) = payload.local_delegate.as_ref() {
        cfg.validate()?;
    }

    let mut parent_feedback = match payload
        .parent_feedback_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        Some(parent_id) => Some(
            store
                .load_feedback(parent_id)
                .map_err(store_error)?
                .ok_or_else(|| format!("feedback revision parent not found: {parent_id}"))?,
        ),
        None => None,
    };

    let now = current_time_unix_ms();
    let id = format!("fb-{now}");

    let mut budget = AttachmentBudget::new();

    let screens = persist_storyboard_screens(&store, &id, &payload, &mut budget)?;
    let has_storyboard = !screens.is_empty();

    let screenshot_filename = match payload.screenshot_base64.as_deref() {
        _ if has_storyboard => screens
            .first()
            .and_then(|screen| screen.screenshot_filename.clone()),
        Some(b64) if !b64.is_empty() => {
            let bytes = decode_base64("screenshot", b64)?;
            budget.add("screenshot", bytes.len())?;
            Some(
                store
                    .save_attachment(&id, "canvas.png", &bytes)
                    .map_err(store_error)?,
            )
        }
        _ => None,
    };

    let pasted_images = if has_storyboard {
        screens
            .first()
            .map(|screen| screen.pasted_images.clone())
            .unwrap_or_default()
    } else {
        let mut images = Vec::with_capacity(payload.pasted_images_base64.len());
        for (idx, b64) in payload.pasted_images_base64.iter().enumerate() {
            if b64.is_empty() {
                continue;
            }
            let name = format!("pasted-image-{idx}");
            let bytes = decode_base64(&name, b64)?;
            budget.add(&name, bytes.len())?;
            let filename = format!("paste-{idx}.png");
            let saved = store
                .save_attachment(&id, &filename, &bytes)
                .map_err(store_error)?;
            images.push(saved);
        }
        images
    };

    let voice_filename = match payload.voice_base64.as_deref() {
        Some(b64) if !b64.is_empty() => {
            let bytes = decode_base64("voice", b64)?;
            budget.add("voice", bytes.len())?;
            let ext = guess_voice_ext(payload.voice_mime_type.as_deref());
            Some(
                store
                    .save_attachment(&id, &format!("voice.{ext}"), &bytes)
                    .map_err(store_error)?,
            )
        }
        _ => None,
    };

    let first_screen = screens.first();
    let mut record = FeedbackRecord {
        id: id.clone(),
        feedback_type: payload.feedback_type,
        status: FeedbackStatus::New,
        page_route: if let Some(route) = first_screen
            .map(|screen| screen.intended_route.as_str())
            .filter(|route| !route.is_empty())
        {
            route.to_string()
        } else if payload.page_route.is_empty() {
            "/".into()
        } else {
            payload.page_route
        },
        feedback_text: payload.feedback_text,
        annotations: first_screen
            .map(|screen| screen.annotations.clone())
            .unwrap_or(payload.annotations),
        pasted_images,
        screenshot_filename,
        screens,
        voice_filename,
        voice_transcript: payload.voice_transcript.filter(|s| !s.trim().is_empty()),
        window_width: payload.window_width,
        window_height: payload.window_height,
        created_at_unix_ms: now,
        updated_at_unix_ms: now,
        lineage_job_id: None,
        local_delegate: payload.local_delegate.clone(),
        revision: parent_feedback
            .as_ref()
            .map(next_feedback_revision)
            .unwrap_or_default(),
    };

    store.save_feedback(&record).map_err(store_error)?;

    // Enqueue a lineage job so reviewers can immediately triage. The
    // agent selection defaults to ClaudeCode so older UI builds (which
    // don't send the field) continue to work unchanged.
    let agent = payload.agent.unwrap_or_default();
    let engine = LineageEngine::new(&store);
    let job = engine
        .enqueue_job_for_feedback(&mut record, agent)
        .map_err(store_error)?;

    // Plan-review gate must be armed BEFORE any run fires — the pipeline
    // reads the flag right after the BackendPlan stage validates.
    let job = if payload.pause_after_plan {
        engine.request_plan_review(&job.id).map_err(store_error)?
    } else {
        job
    };

    // Link the old record to its successor only after the new feedback and
    // lineage job are durable. A metadata-write failure must not make the UI
    // report that an otherwise valid revision was lost.
    if let Some(parent) = parent_feedback.as_mut() {
        parent.revision.superseded_by_feedback_id = Some(record.id.clone());
        parent.updated_at_unix_ms = now;
        if let Err(err) = store.save_feedback(parent) {
            eprintln!(
                "evolvo: revision {} saved but failed to link parent {}: {err}",
                record.id, parent.id
            );
        }
    }

    // Auto-evolve: skip the manual "Evolve" click and fire the agent run
    // immediately. We swallow run-prep errors so the feedback record is
    // still reported as saved — the failure is recorded on the lineage
    // job by `start_implementation_run` itself (note + status=Failed),
    // which the user can then see on the Lineage tab.
    if payload.auto_evolve {
        let _ = start_implementation_run(&store, &engine, &job);
    }

    Ok(record)
}

fn next_feedback_revision(parent: &FeedbackRecord) -> FeedbackRevision {
    FeedbackRevision {
        number: parent.revision.number.max(1).saturating_add(1),
        parent_feedback_id: Some(parent.id.clone()),
        superseded_by_feedback_id: None,
    }
}

fn persist_storyboard_screens(
    store: &crate::store::Store,
    feedback_id: &str,
    payload: &SubmitFeedbackPayload,
    budget: &mut AttachmentBudget,
) -> Result<Vec<FeedbackScreenRecord>, String> {
    let mut records = Vec::with_capacity(payload.screens.len());
    for (screen_idx, screen) in payload.screens.iter().enumerate() {
        let ordinal = screen_idx + 1;
        let screenshot_filename = match screen.screenshot_base64.as_deref() {
            Some(b64) if !b64.is_empty() => {
                let label = format!("screen-{ordinal:02}-screenshot");
                let bytes = decode_base64(&label, b64)?;
                budget.add(&label, bytes.len())?;
                Some(
                    store
                        .save_attachment(feedback_id, &format!("screen-{ordinal:02}.png"), &bytes)
                        .map_err(store_error)?,
                )
            }
            _ => None,
        };

        let mut pasted_images = Vec::with_capacity(screen.pasted_images_base64.len());
        for (paste_idx, b64) in screen.pasted_images_base64.iter().enumerate() {
            if b64.is_empty() {
                continue;
            }
            let label = format!("screen-{ordinal:02}-pasted-image-{paste_idx}");
            let bytes = decode_base64(&label, b64)?;
            budget.add(&label, bytes.len())?;
            let saved = store
                .save_attachment(
                    feedback_id,
                    &format!("screen-{ordinal:02}-paste-{paste_idx}.png"),
                    &bytes,
                )
                .map_err(store_error)?;
            pasted_images.push(saved);
        }

        records.push(FeedbackScreenRecord {
            id: if screen.id.trim().is_empty() {
                format!("screen-{ordinal}")
            } else {
                screen.id.clone()
            },
            name: if screen.name.trim().is_empty() {
                format!("Screen {ordinal}")
            } else {
                screen.name.trim().to_string()
            },
            description: screen.description.trim().to_string(),
            intended_route: if screen.intended_route.trim().is_empty() {
                format!("/screen-{ordinal}")
            } else {
                screen.intended_route.trim().to_string()
            },
            annotations: screen.annotations.clone(),
            pasted_images,
            screenshot_filename,
            window_width: screen.window_width,
            window_height: screen.window_height,
        });
    }
    Ok(records)
}

/// Enumerate every known agent backend along with whether its CLI binary is
/// present on PATH. The UI uses this to grey out chips for agents the user
/// hasn't installed. Results are cached in-process (see `agent::is_installed`)
/// so repeated polls from the feedback panel are cheap.
#[tauri::command]
pub fn list_available_agents() -> Result<Vec<AgentAvailability>, String> {
    Ok(agent::availability())
}

/// Whether the local-model delegation worker (`pi` + Ollama) is wireable
/// on this host. The UI uses this to gate the "Local worker" chip in the
/// feedback panel; a `pi_installed: false` or `ollama_reachable: false`
/// result greys the chip with a tooltip telling the user which piece to
/// install. Cached per-process — same restart-to-rescan convention as
/// `list_available_agents`.
#[tauri::command]
pub fn list_local_delegate_availability() -> Result<LocalDelegateAvailability, String> {
    Ok(agent::local_delegate_availability())
}

/// Aggregate the per-job pi-events.jsonl stream into a `DelegationSummary`
/// the UI can render in the Lineage detail panel. Returns `Ok(None)` when
/// the events file is absent (the steady state for jobs that didn't opt
/// into delegation), so the UI hides the "Local worker" section
/// completely instead of rendering a row of zeros.
#[tauri::command]
pub fn read_delegation_summary(
    state: State<'_, AppState>,
    payload: EntityIdPayload,
) -> Result<Option<DelegationSummary>, String> {
    let root = state.store().layout().root().to_path_buf();
    let job_dir = runner::job_workspace_dir(&root, &payload.id);
    crate::delegation::read_summary(&job_dir).map_err(|e| format!("read pi-events: {e}"))
}

#[tauri::command]
pub fn list_feedback(state: State<'_, AppState>) -> Result<Vec<FeedbackRecord>, String> {
    state.store().list_feedback().map_err(store_error)
}

#[tauri::command]
pub fn load_feedback(
    state: State<'_, AppState>,
    payload: EntityIdPayload,
) -> Result<Option<FeedbackRecord>, String> {
    state
        .store()
        .load_feedback(&payload.id)
        .map_err(store_error)
}

#[tauri::command]
pub fn load_feedback_for_revision(
    state: State<'_, AppState>,
    payload: EntityIdPayload,
) -> Result<Option<EditableFeedback>, String> {
    let store = state.store();
    let Some(feedback) = store.load_feedback(&payload.id).map_err(store_error)? else {
        return Ok(None);
    };

    let screens = if feedback.screens.is_empty() {
        vec![editable_screen(
            &store,
            &feedback.id,
            "screen-1",
            "Screen 1",
            "",
            &feedback.page_route,
            &feedback.annotations,
            &feedback.pasted_images,
            feedback.window_width,
            feedback.window_height,
        )?]
    } else {
        feedback
            .screens
            .iter()
            .map(|screen| {
                editable_screen(
                    &store,
                    &feedback.id,
                    &screen.id,
                    &screen.name,
                    &screen.description,
                    &screen.intended_route,
                    &screen.annotations,
                    &screen.pasted_images,
                    screen.window_width,
                    screen.window_height,
                )
            })
            .collect::<Result<Vec<_>, _>>()?
    };

    Ok(Some(EditableFeedback { feedback, screens }))
}

#[allow(clippy::too_many_arguments)]
fn editable_screen(
    store: &crate::store::Store,
    feedback_id: &str,
    id: &str,
    name: &str,
    description: &str,
    intended_route: &str,
    annotations: &[serde_json::Value],
    pasted_images: &[String],
    window_width: u32,
    window_height: u32,
) -> Result<EditableFeedbackScreen, String> {
    let pasted_image_data_urls = pasted_images
        .iter()
        .map(|filename| {
            let bytes = store
                .read_attachment(feedback_id, filename)
                .map_err(store_error)?;
            Ok(bytes
                .map(|bytes| {
                    format!(
                        "data:{};base64,{}",
                        image_mime_type(&bytes),
                        STANDARD.encode(bytes)
                    )
                })
                .unwrap_or_default())
        })
        .collect::<Result<Vec<_>, String>>()?;

    Ok(EditableFeedbackScreen {
        id: id.to_string(),
        name: name.to_string(),
        description: description.to_string(),
        intended_route: intended_route.to_string(),
        annotations: annotations.to_vec(),
        pasted_image_data_urls,
        window_width,
        window_height,
    })
}

fn image_mime_type(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "image/gif"
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else {
        "application/octet-stream"
    }
}

#[tauri::command]
pub fn delete_feedback(
    state: State<'_, AppState>,
    payload: EntityIdPayload,
) -> Result<bool, String> {
    state
        .store()
        .delete_feedback(&payload.id)
        .map_err(store_error)
}

#[tauri::command]
pub fn list_lineage_jobs(state: State<'_, AppState>) -> Result<Vec<LineageJobRecord>, String> {
    state.store().list_lineage_jobs().map_err(store_error)
}

#[tauri::command]
pub fn load_lineage_job(
    state: State<'_, AppState>,
    payload: EntityIdPayload,
) -> Result<Option<LineageJobRecord>, String> {
    state
        .store()
        .load_lineage_job(&payload.id)
        .map_err(store_error)
}

#[tauri::command]
pub fn list_job_stages(
    state: State<'_, AppState>,
    payload: EntityIdPayload,
) -> Result<Vec<crate::types::StageState>, String> {
    let job = state
        .store()
        .load_lineage_job(&payload.id)
        .map_err(store_error)?
        .ok_or_else(|| format!("lineage job not found: {}", payload.id))?;
    Ok(job.stages)
}

#[tauri::command]
pub fn read_job_plan(
    state: State<'_, AppState>,
    payload: EntityIdPayload,
) -> Result<Option<serde_json::Value>, String> {
    let root = state.store().layout().root().to_path_buf();
    let job_dir = runner::job_workspace_dir(&root, &payload.id);
    match crate::plan::load_plan(&job_dir).map_err(store_error)? {
        Some(p) => Ok(Some(
            serde_json::to_value(p).map_err(|e| format!("serialize plan: {e}"))?,
        )),
        None => Ok(None),
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TailStageLogPayload {
    pub id: String,
    pub stage: String,
    #[serde(default)]
    pub max_bytes: Option<usize>,
}

#[tauri::command]
pub fn tail_stage_log(
    state: State<'_, AppState>,
    payload: TailStageLogPayload,
) -> Result<String, String> {
    let root = state.store().layout().root().to_path_buf();
    let job_dir = runner::job_workspace_dir(&root, &payload.id);
    let safe = payload
        .stage
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect::<String>();
    if safe.is_empty() {
        return Err("stage slug empty".into());
    }
    let log = job_dir.join("logs").join(format!("{safe}.log"));
    if !log.exists() {
        return Ok(String::new());
    }
    let bytes = std::fs::read(&log).map_err(|e| format!("read log: {e}"))?;
    let cap = payload.max_bytes.unwrap_or(32 * 1024);
    let start = bytes.len().saturating_sub(cap);
    Ok(String::from_utf8_lossy(&bytes[start..]).into_owned())
}

/// Re-enter the multi-stage pipeline for a job whose previous run
/// failed or was interrupted. Reuses the existing worktree and plan.json;
/// stages already green (or whose output is already recorded in
/// `plan.stage`) are skipped — only the first non-green stage is
/// re-dispatched to Claude. Returns the refreshed job record.
#[tauri::command]
pub fn resume_lineage_job(
    state: State<'_, AppState>,
    payload: EntityIdPayload,
) -> Result<LineageJobRecord, String> {
    let store = state.store();
    let engine = LineageEngine::new(&store);

    let job = store
        .load_lineage_job(&payload.id)
        .map_err(store_error)?
        .ok_or_else(|| format!("lineage job not found: {}", payload.id))?;

    // Status guard: resuming a job whose runner is still alive would spawn
    // a SECOND pipeline thread into the same worktree — two agents editing
    // the same files. Only terminal-but-recoverable states may resume.
    if matches!(
        job.status,
        LineageJobStatus::Implementing | LineageJobStatus::Promoted | LineageJobStatus::Merging
    ) {
        return Err(format!(
            "cannot resume a job in status {:?} — wait for the current run to finish or fail",
            job.status
        ));
    }

    // Flip to `Implementing` synchronously *before* spawning the resume
    // thread so the UI sees the change immediately. (Engine writes are
    // additionally serialised by a per-job lock — see
    // `lineage::job_write_lock` — so thread/main interleavings can no
    // longer drop updates.)
    let updated = engine
        .force_status(&job.id, LineageJobStatus::Implementing)
        .map_err(store_error)?;

    runner::resume_pipeline(store.clone(), job.id.clone())?;

    Ok(updated)
}

/// "Advance" button entry point. Behaviour depends on the job's current
/// status:
/// - `Pending` → fork the source repo into a lineage worktree, spawn
///   `claude -p … --dangerously-skip-permissions` in it, and return with
///   status `Implementing`. The run continues on a background thread;
///   the job will flip to `BuildReady` or `Failed` when claude exits.
/// - `Planned` (with the plan-review gate armed) → resume implementation.
/// - `BuildReady` → flip to `Merging` synchronously and run the real
///   promotion merge (see `promotion::start_promotion`) on a background
///   thread. The job lands on `Promoted` on success or bounces back to
///   `BuildReady` with an explanatory note on any refusal/failure.
/// - anything else → the normal state-machine transition, which will
///   surface an error if invalid.
#[tauri::command]
pub fn approve_lineage_job(
    state: State<'_, AppState>,
    payload: EntityIdPayload,
) -> Result<LineageJobRecord, String> {
    let store = state.store();
    let engine = LineageEngine::new(&store);

    let job = store
        .load_lineage_job(&payload.id)
        .map_err(store_error)?
        .ok_or_else(|| format!("lineage job not found: {}", payload.id))?;
    match job.status {
        LineageJobStatus::Pending => start_implementation_run(&store, &engine, &job),
        // Plan-review gate: a job parked in `Planned` by the pipeline
        // resumes implementation on approval instead of jumping straight
        // to Promoted. Only fires for jobs that requested the gate —
        // legacy Planned jobs keep the historical approve semantics.
        LineageJobStatus::Planned if job.pause_after_plan => {
            engine.mark_plan_approved(&job.id).map_err(store_error)?;
            let _ = engine.append_note(
                &job.id,
                "plan approved by reviewer — resuming pipeline into implementation",
            );
            let updated = engine
                .force_status(&job.id, LineageJobStatus::Implementing)
                .map_err(store_error)?;
            runner::resume_pipeline(store.clone(), job.id.clone())?;
            Ok(updated)
        }
        LineageJobStatus::BuildReady => {
            let updated = engine
                .force_status(&job.id, LineageJobStatus::Merging)
                .map_err(store_error)?;
            let _ = engine.append_note(&job.id, "reviewer approved — promotion starting");
            crate::promotion::start_promotion(store.clone(), job.id.clone());
            Ok(updated)
        }
        _ => engine
            .transition(&payload.id, Transition::Approve)
            .map_err(store_error),
    }
}

fn start_implementation_run(
    store: &crate::store::Store,
    engine: &LineageEngine<'_>,
    job: &LineageJobRecord,
) -> Result<LineageJobRecord, String> {
    // Must have the linked feedback to build a useful prompt.
    let feedback = store
        .load_feedback(&job.feedback_id)
        .map_err(store_error)?
        .ok_or_else(|| format!("feedback {} not found for job {}", job.feedback_id, job.id))?;

    let prepared = runner::prepare_run(store, job, &feedback, job.agent).map_err(|e| {
        // Record the failure on the job so the reviewer sees why nothing
        // happened, then surface the error to the frontend.
        let msg = store_error(e);
        let _ = engine.append_note(&job.id, &format!("failed to prepare run: {msg}"));
        crate::lineage::force_status_with_retry(engine, &job.id, LineageJobStatus::Failed);
        msg
    })?;

    // Persist artifact paths + flip to Implementing BEFORE spawning the
    // background thread so the UI reflects reality if the process crashes.
    let _ = engine
        .set_run_artifacts(
            &job.id,
            prepared.worktree.display().to_string(),
            prepared.branch.clone(),
            prepared.log_file.display().to_string(),
            prepared.source_repo.display().to_string(),
            prepared.base_branch.clone(),
        )
        .map_err(store_error)?;
    let updated = engine
        .force_status(&job.id, LineageJobStatus::Implementing)
        .map_err(store_error)?;

    if runner::is_multi_stage_candidate(&feedback) {
        runner::launch_pipeline(store.clone(), job.id.clone(), feedback, prepared, job.agent);
    } else {
        runner::launch_agent_session(store.clone(), job.id.clone(), prepared, job.agent);
    }
    Ok(updated)
}

#[tauri::command]
pub fn reject_lineage_job(
    state: State<'_, AppState>,
    payload: EntityIdPayload,
) -> Result<LineageJobRecord, String> {
    let store = state.store();
    let engine = LineageEngine::new(&store);
    engine
        .transition(&payload.id, Transition::Reject)
        .map_err(store_error)
}

/// Re-run the implementation for a job that previously failed or got stuck.
/// Destroys the prior worktree + branch + job workspace so the fresh run
/// starts from the current HEAD of the source repo, then dispatches through
/// the same pipeline as the first Advance. Only valid when the job is in a
/// state where a retry is meaningful (see `LineageJobStatus::can_retry`).
#[tauri::command]
pub fn retry_lineage_job(
    state: State<'_, AppState>,
    payload: EntityIdPayload,
) -> Result<LineageJobRecord, String> {
    let store = state.store();
    let engine = LineageEngine::new(&store);

    // Serialize the whole soft-retry decision (checkpoint lookup, worktree
    // reset/teardown, status flip) per job id. Without this, two
    // concurrent retry calls on the same job (e.g. a double click, or a
    // UI retry racing a background reconciler) can both read the same
    // `can_retry()`-eligible status, then interleave their worktree
    // resets/teardowns — one retry's `reset_worktree_to` can run against
    // a worktree the other retry's `cleanup_previous_run` just deleted
    // (TOCTOU between the `.exists()` filter below and the actual git
    // operations a few lines later). Holding this lock for the duration
    // turns that race into a clean second-caller failure (`can_retry()`
    // false, or a "no longer exists" error from `reset_worktree_to`)
    // instead of undefined worktree state.
    //
    // This is `job_retry_lock`, NOT `job_write_lock` — this function calls
    // `engine.append_note` / `engine.force_status` below, which internally
    // take `job_write_lock`; reusing that lock here would self-deadlock
    // since `std::sync::Mutex` isn't reentrant.
    let _retry_guard = crate::lineage::job_retry_lock(&payload.id);
    let _retry_guard = _retry_guard.lock().unwrap_or_else(|p| p.into_inner());

    let job = store
        .load_lineage_job(&payload.id)
        .map_err(store_error)?
        .ok_or_else(|| format!("lineage job not found: {}", payload.id))?;
    if !job.status.can_retry() {
        return Err(format!(
            "job {} is in status {:?} which does not support retry",
            job.id, job.status
        ));
    }

    let workspace_root = store.layout().root().to_path_buf();

    // Soft retry: when the multi-stage pipeline already banked green
    // checkpoints in this worktree, reset to the last one and resume —
    // the green stages' work survives and the Reflexion loop re-attacks
    // only the failed stage. Far cheaper than rebuilding the worktree.
    // Falls through to the destructive path when there's no checkpoint
    // (legacy runs, or a pipeline that never went green) or the reset
    // itself fails.
    if let Some(worktree) = job
        .worktree_path
        .as_ref()
        .map(std::path::PathBuf::from)
        .filter(|p| p.exists())
    {
        let job_dir = runner::job_workspace_dir(&workspace_root, &job.id);
        if job_dir.join(crate::plan::PLAN_FILENAME).exists() {
            if let Some(sha) = runner::last_green_checkpoint(&worktree) {
                match runner::reset_worktree_to(&worktree, &sha) {
                    Ok(()) => {
                        let _ = engine.append_note(
                            &job.id,
                            &format!(
                                "retry: worktree reset to last green checkpoint {} — resuming pipeline (green stages keep their work)",
                                &sha[..sha.len().min(8)]
                            ),
                        );
                        let updated = engine
                            .force_status(&job.id, LineageJobStatus::Implementing)
                            .map_err(store_error)?;
                        runner::resume_pipeline(store.clone(), job.id.clone())?;
                        return Ok(updated);
                    }
                    Err(e) => {
                        let _ = engine.append_note(
                            &job.id,
                            &format!("retry: checkpoint reset failed ({e}) — falling back to full teardown"),
                        );
                    }
                }
            }
        }
    }

    let source = runner::resolve_source_repo().ok_or_else(|| {
        "could not locate Evolvo source repo — set EVOLVO_SOURCE_REPO or run from within the repo"
            .to_string()
    })?;
    runner::cleanup_previous_run(&source, &workspace_root, &job.id).map_err(|e| {
        let msg = store_error(e);
        let _ = engine.append_note(&job.id, &format!("retry cleanup failed: {msg}"));
        msg
    })?;

    // Drop a breadcrumb so the reviewer can tell this is attempt N rather
    // than the original run. `start_implementation_run` will overwrite the
    // worktree/branch/log pointers when `prepare_run` succeeds below.
    let _ = engine.append_note(&job.id, "retry requested — previous run torn down");

    let refreshed = store
        .load_lineage_job(&job.id)
        .map_err(store_error)?
        .ok_or_else(|| format!("lineage job not found after cleanup: {}", job.id))?;
    start_implementation_run(&store, &engine, &refreshed)
}

/// Launch the app built in a lineage job's worktree. Only valid after the
/// job has reached a state where a worktree exists and the agent has
/// finished writing code (see `LineageJobStatus::can_run`). The spawned
/// process runs in the background with its own `EVOLVO_WORKSPACE_ROOT` so it
/// cannot see or mutate the host Evolvo's workspace.
#[tauri::command]
pub fn run_lineage_job(
    state: State<'_, AppState>,
    payload: EntityIdPayload,
) -> Result<LineageJobRecord, String> {
    let store = state.store();
    let engine = LineageEngine::new(&store);

    let job = store
        .load_lineage_job(&payload.id)
        .map_err(store_error)?
        .ok_or_else(|| format!("lineage job not found: {}", payload.id))?;

    if !job.status.can_run() {
        return Err(format!(
            "job {} is in status {:?} — Advance it first before running the iteration",
            job.id, job.status
        ));
    }
    if job.worktree_path.is_none() {
        return Err(format!(
            "job {} has no worktree yet — Advance it first",
            job.id
        ));
    }

    let _ = engine.append_note(&job.id, "run requested — spawning iteration app");
    runner::launch_iteration_run(store.clone(), job.id.clone());

    // Return the current record (with the note appended) so the UI can
    // refresh without a second round-trip.
    store
        .load_lineage_job(&payload.id)
        .map_err(store_error)?
        .ok_or_else(|| format!("lineage job disappeared after run: {}", payload.id))
}

/// Read-only dry-run preview of what an `Evolve` click will dispatch. The
/// UI calls this when the reviewer presses Evolve so they can confirm the
/// upcoming iteration's planned scope *before* the state machine actually
/// transitions. Per architect doc I3 (bounded blast radius / dry-run
/// default) this command MUST NOT mutate state — it only reads
/// `plan.json` + the persisted job record. If `plan.json` is absent
/// (e.g. classic single-stage feedback that hasn't planned yet, or an
/// older job that pre-dates the multi-stage pipeline) the summary
/// returns a `PreviewSummary` with empty `plannedFiles` and an
/// informational `note`, rather than erroring — that way the reviewer
/// can still proceed "agent unguided" without the dry-run blocking
/// I-P1 (lineage always works).
#[tauri::command]
pub fn preview_lineage_evolution(
    state: State<'_, AppState>,
    payload: EntityIdPayload,
) -> Result<PreviewSummary, String> {
    preview_lineage_evolution_impl(state.store(), &payload.id)
}

/// Testable body of `preview_lineage_evolution`. Lives separately so unit
/// tests can drive it against a `tempdir`-backed `Store` without going
/// through the `#[tauri::command]` shim's `State<'_, AppState>` plumbing.
pub fn preview_lineage_evolution_impl(
    store: crate::store::Store,
    job_id: &str,
) -> Result<PreviewSummary, String> {
    let job = store
        .load_lineage_job(job_id)
        .map_err(store_error)?
        .ok_or_else(|| format!("lineage job not found: {job_id}"))?;

    let target_iteration = job.iteration;
    let source_iteration = target_iteration.saturating_sub(1);
    let target_port = runner::iteration_port(target_iteration);

    let workspace_root = store.layout().root().to_path_buf();
    let job_dir = runner::job_workspace_dir(&workspace_root, &job.id);

    let mut notes: Vec<String> = Vec::new();
    let (plan_summary, planned_files) = match crate::plan::load_plan(&job_dir) {
        Ok(Some(plan)) => synthesize_preview(&plan, &mut notes),
        Ok(None) => {
            notes.push(
                "No plan recorded yet — proceeding will run the agent without a recorded plan."
                    .to_string(),
            );
            (default_plan_summary(&job), Vec::new())
        }
        Err(e) => {
            notes.push(format!("plan.json could not be read: {e}"));
            (default_plan_summary(&job), Vec::new())
        }
    };

    Ok(PreviewSummary {
        job_id: job.id,
        agent: job.agent,
        source_iteration,
        target_iteration,
        target_port,
        plan_summary,
        planned_files,
        notes,
        captured_at_unix_ms: current_time_unix_ms(),
    })
}

const PLAN_SUMMARY_BUDGET: usize = 4096;
const MAX_PLANNED_FILES: usize = 64;

fn default_plan_summary(job: &LineageJobRecord) -> String {
    let mut lines: Vec<String> = Vec::new();
    if !job.title.is_empty() {
        lines.push(job.title.clone());
    }
    if !job.summary.is_empty() {
        lines.push(job.summary.clone());
    }
    if lines.is_empty() {
        lines.push(format!(
            "Lineage job {} — agent {} will run from the staged feedback.",
            job.id,
            job.agent.label()
        ));
    }
    truncate(&lines.join("\n"), PLAN_SUMMARY_BUDGET)
}

fn synthesize_preview(
    plan: &crate::plan::IterationPlan,
    notes: &mut Vec<String>,
) -> (String, Vec<String>) {
    let mut summary_lines: Vec<String> = Vec::new();
    if !plan.app.name.is_empty() {
        summary_lines.push(format!("App: {}", plan.app.name));
    }
    if !plan.app.one_liner.is_empty() {
        summary_lines.push(format!("Goal: {}", plan.app.one_liner));
    }
    summary_lines.push(format!("Stage: {}", plan.stage.label()));

    let entity_count = plan.backend.entities.len();
    let command_count = plan.backend.commands.len();
    let route_count = plan.frontend.routes.len();
    let component_count = plan.frontend.components.len();
    let scenario_count = plan.e2e.scenarios.len();
    summary_lines.push(format!(
        "Backend: {entity_count} entities · {command_count} commands · {} tests",
        plan.backend.tests.len()
    ));
    summary_lines.push(format!(
        "Frontend: {route_count} routes · {component_count} components"
    ));
    summary_lines.push(format!("E2E: {scenario_count} scenarios"));

    let mut files: Vec<String> = Vec::new();
    for e in &plan.backend.entities {
        files.push(format!("entity: {}", e.name));
    }
    for c in &plan.backend.commands {
        files.push(format!("command: {} → {}", c.name, c.output));
    }
    for r in &plan.frontend.routes {
        files.push(format!("route: {} ({})", r.path, r.component));
    }
    for c in &plan.frontend.components {
        files.push(format!("component: {}", c.name));
    }
    for s in &plan.e2e.scenarios {
        files.push(format!("e2e: {}", s.title));
    }
    if files.len() > MAX_PLANNED_FILES {
        let dropped = files.len() - MAX_PLANNED_FILES;
        files.truncate(MAX_PLANNED_FILES);
        notes.push(format!(
            "planned items truncated for display ({dropped} more)"
        ));
    }

    if files.is_empty() && plan.history.is_empty() {
        notes.push(
            "plan.json present but no concrete artefacts planned yet — agent will plan as it runs."
                .to_string(),
        );
    }

    (
        truncate(&summary_lines.join("\n"), PLAN_SUMMARY_BUDGET),
        files,
    )
}

fn truncate(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while !s.is_char_boundary(end) && end > 0 {
        end -= 1;
    }
    let mut out = s[..end].to_string();
    out.push_str("\n…(truncated)");
    out
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotePayload {
    pub id: String,
    pub note: String,
}

#[tauri::command]
pub fn append_lineage_note(
    state: State<'_, AppState>,
    payload: NotePayload,
) -> Result<LineageJobRecord, String> {
    let store = state.store();
    let engine = LineageEngine::new(&store);
    engine
        .append_note(&payload.id, &payload.note)
        .map_err(store_error)
}

#[tauri::command]
pub fn open_workspace_path(state: State<'_, AppState>) -> Result<String, String> {
    Ok(state.workspace_root_display())
}

/// Capture the Tauri window's content as a PNG and return base64.
///
/// The UI calls this right before submitting feedback so it can composite
/// annotations onto the real page view — otherwise the agent reviewing the
/// feedback only sees the transparent strokes from the canvas overlay and
/// has no visual reference for what the user was annotating.
///
/// Matching is done by window title (set in `tauri.conf.json`); if multiple
/// windows share the title the first match wins. We take the whole window
/// including the native title bar because cropping needs platform-specific
/// DPI handling — the UI can trim if needed.
#[tauri::command]
pub fn capture_window_png(window: tauri::WebviewWindow) -> Result<String, String> {
    let title = window.title().map_err(|e| e.to_string())?;
    let windows = xcap::Window::all().map_err(|e| e.to_string())?;
    let target = windows
        .into_iter()
        .find(|w| w.title().ok().as_deref() == Some(title.as_str()))
        .ok_or_else(|| format!("no OS window matching title {title:?}"))?;
    let rgba = target.capture_image().map_err(|e| e.to_string())?;
    let mut buf: Vec<u8> = Vec::new();
    let dyn_img = image::DynamicImage::ImageRgba8(rgba);
    dyn_img
        .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(STANDARD.encode(&buf))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportLineagePayload {
    pub id: String,
    /// Optional override for where the `.evolvo-bundle` file is written. When
    /// absent, exports land in `{workspace_root}/exports/`. Path is taken as
    /// given (no shell expansion) — the UI is responsible for resolving `~`.
    #[serde(default)]
    pub destination_dir: Option<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportLineageResult {
    pub bundle_path: String,
}

/// Operationalises product invariant I-P4 — write a portable `.evolvo-bundle`
/// archive for a given lineage job. The bundle contains the job, the feedback
/// row that fed it, and every attachment, with all host-specific paths
/// stripped. See `bundle.rs` for the file layout and the round-trip test.
#[tauri::command]
pub fn export_lineage(
    state: State<'_, AppState>,
    payload: ExportLineagePayload,
) -> Result<ExportLineageResult, String> {
    let store = state.store();
    let dest = match payload.destination_dir.as_deref() {
        Some(d) if !d.trim().is_empty() => std::path::PathBuf::from(d),
        _ => store.layout().root().join("exports"),
    };
    let path = bundle::export_lineage_to_dir(&store, &payload.id, &dest, APP_NAME, APP_VERSION)
        .map_err(store_error)?;
    Ok(ExportLineageResult {
        bundle_path: path.display().to_string(),
    })
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportLineagePayload {
    pub bundle_path: String,
    /// Workspace root for the new app. Must be either non-existent or empty
    /// of lineage data — see `bundle::import_lineage_bundle`.
    pub target_workspace_root: String,
}

/// Inverse of `export_lineage` — read a `.evolvo-bundle` and seed a fresh
/// workspace. Does **not** mutate the host's currently-mounted workspace.
/// The UI is expected to spawn (or instruct the user to spawn) a new Evolvo
/// process with `EVOLVO_WORKSPACE_ROOT=<target>` set, since the running
/// instance is locked to its own root.
#[tauri::command]
pub fn import_lineage_bundle(payload: ImportLineagePayload) -> Result<ImportSummary, String> {
    let bundle_path = std::path::PathBuf::from(&payload.bundle_path);
    let target_root = std::path::PathBuf::from(&payload.target_workspace_root);
    bundle::import_lineage_bundle(&bundle_path, &target_root).map_err(store_error)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenUrlPayload {
    pub url: String,
}

/// Open an external URL in the user's default browser. Restricted to http(s)
/// so we can't be tricked into shelling out to arbitrary `file://` or custom
/// schemes. We shell out to the platform's native "open" instead of pulling
/// in `tauri-plugin-opener` just for one Star-Us button — fewer dependencies,
/// no extra capability grants.
#[tauri::command]
pub fn open_external_url(payload: OpenUrlPayload) -> Result<(), String> {
    let url = payload.url;
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(format!("refusing to open non-http(s) url: {url}"));
    }

    #[cfg(target_os = "macos")]
    let spawn_result = std::process::Command::new("open").arg(&url).spawn();

    #[cfg(target_os = "linux")]
    let spawn_result = std::process::Command::new("xdg-open").arg(&url).spawn();

    #[cfg(target_os = "windows")]
    let spawn_result = std::process::Command::new("cmd")
        .args(["/C", "start", "", &url])
        .spawn();

    spawn_result
        .map(|_| ())
        .map_err(|e| format!("failed to open url {url}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FeedbackScreenPayload, FeedbackType};
    use tempfile::tempdir;

    fn mk_payload(text: &str, pasted: usize, with_screenshot: bool) -> SubmitFeedbackPayload {
        let tiny_png = STANDARD.encode([137u8, 80, 78, 71, 13, 10, 26, 10]);
        SubmitFeedbackPayload {
            feedback_type: FeedbackType::Bug,
            page_route: "/".into(),
            feedback_text: text.into(),
            annotations: vec![],
            pasted_images_base64: (0..pasted).map(|_| tiny_png.clone()).collect(),
            screenshot_base64: if with_screenshot {
                Some(tiny_png)
            } else {
                None
            },
            screens: vec![],
            voice_base64: None,
            voice_mime_type: None,
            voice_transcript: None,
            window_width: 1024,
            window_height: 768,
            agent: None,
            auto_evolve: false,
            pause_after_plan: false,
            local_delegate: None,
            parent_feedback_id: None,
        }
    }

    fn state_with_tmp() -> (tempfile::TempDir, AppState) {
        let temp = tempdir().unwrap();
        let state = AppState::with_root(temp.path().to_path_buf());
        (temp, state)
    }

    #[test]
    fn estimated_decoded_len_matches_actual_decode() {
        for len in [0usize, 3, 8, 1024, 1_000_000] {
            let bytes = vec![7u8; len];
            let b64 = STANDARD.encode(&bytes);
            let estimate = estimated_decoded_len(&b64);
            // The pre-decode estimate must never be smaller than the real
            // decoded size, or an attacker could sneak an oversized payload
            // past the cheap check and force a large allocation anyway.
            assert!(
                estimate >= len,
                "estimate {estimate} underestimated actual decoded len {len}"
            );
            // And it should be a tight bound, not wildly over.
            assert!(
                estimate - len <= 2,
                "estimate {estimate} too loose for {len}"
            );
        }
    }

    #[test]
    fn check_encoded_size_rejects_before_decoding() {
        let oversized = STANDARD.encode(vec![0u8; MAX_ATTACHMENT_BYTES + 1]);
        let err = check_encoded_size("screenshot", &oversized).unwrap_err();
        assert!(err.contains("screenshot"));
        assert!(err.contains(&MAX_ATTACHMENT_BYTES.to_string()));

        let ok = STANDARD.encode(vec![0u8; 1024]);
        assert!(check_encoded_size("screenshot", &ok).is_ok());
    }

    #[test]
    fn decode_base64_rejects_oversized_attachment() {
        let oversized = STANDARD.encode(vec![0u8; MAX_ATTACHMENT_BYTES + 1]);
        let err = decode_base64("voice", &oversized).unwrap_err();
        assert!(err.contains("voice"));
        assert!(err.to_lowercase().contains("exceeds"));
    }

    #[test]
    fn attachment_budget_rejects_aggregate_over_cap() {
        let mut budget = AttachmentBudget::new();
        // Two attachments, each under the per-attachment cap, whose sum
        // exceeds the aggregate cap.
        let half = MAX_TOTAL_ATTACHMENT_BYTES / 2 + 1;
        budget.add("first", half).unwrap();
        let err = budget.add("second", half).unwrap_err();
        assert!(err.contains("second"));
        assert!(err.contains(&MAX_TOTAL_ATTACHMENT_BYTES.to_string()));
    }

    #[test]
    fn attachment_budget_allows_normal_totals() {
        let mut budget = AttachmentBudget::new();
        budget.add("screenshot", 2 * 1024 * 1024).unwrap();
        budget.add("pasted-image-0", 1024 * 1024).unwrap();
        budget.add("voice", 512 * 1024).unwrap();
    }

    /// Replicates `submit_feedback`'s attachment pipeline (decode + budget +
    /// `Store::save_attachment`) directly against a tempdir-backed `Store`,
    /// since the `#[tauri::command]` shim needs a live `State<'_, AppState>`
    /// that isn't practical to construct in a unit test (see the project's
    /// "test the body, not the shim" convention).
    fn run_attachment_pipeline(
        store: &crate::store::Store,
        id: &str,
        payload: &SubmitFeedbackPayload,
    ) -> Result<(Option<String>, Vec<String>, Option<String>), String> {
        let mut budget = AttachmentBudget::new();

        let screenshot_filename = match payload.screenshot_base64.as_deref() {
            Some(b64) if !b64.is_empty() => {
                let bytes = decode_base64("screenshot", b64)?;
                budget.add("screenshot", bytes.len())?;
                Some(
                    store
                        .save_attachment(id, "canvas.png", &bytes)
                        .map_err(store_error)?,
                )
            }
            _ => None,
        };

        let mut pasted_images = Vec::new();
        for (idx, b64) in payload.pasted_images_base64.iter().enumerate() {
            if b64.is_empty() {
                continue;
            }
            let name = format!("pasted-image-{idx}");
            let bytes = decode_base64(&name, b64)?;
            budget.add(&name, bytes.len())?;
            let filename = format!("paste-{idx}.png");
            let saved = store
                .save_attachment(id, &filename, &bytes)
                .map_err(store_error)?;
            pasted_images.push(saved);
        }

        let voice_filename = match payload.voice_base64.as_deref() {
            Some(b64) if !b64.is_empty() => {
                let bytes = decode_base64("voice", b64)?;
                budget.add("voice", bytes.len())?;
                Some(
                    store
                        .save_attachment(id, "voice.webm", &bytes)
                        .map_err(store_error)?,
                )
            }
            _ => None,
        };

        Ok((screenshot_filename, pasted_images, voice_filename))
    }

    #[test]
    fn submit_feedback_pipeline_rejects_oversized_single_attachment() {
        let (_temp, app) = state_with_tmp();
        let store = app.store();

        let mut payload = mk_payload("hi", 0, false);
        payload.screenshot_base64 = Some(STANDARD.encode(vec![0u8; MAX_ATTACHMENT_BYTES + 1]));

        let err = run_attachment_pipeline(&store, "fb-oversize", &payload).unwrap_err();
        assert!(err.contains("screenshot"));
        assert!(store
            .read_attachment("fb-oversize", "canvas.png")
            .unwrap()
            .is_none());
    }

    #[test]
    fn submit_feedback_pipeline_rejects_oversized_aggregate() {
        let (_temp, app) = state_with_tmp();
        let store = app.store();

        // Five pasted images, each safely under the 8 MiB per-attachment
        // cap, but summing past the 32 MiB aggregate cap.
        let chunk = 7 * 1024 * 1024;
        assert!(chunk < MAX_ATTACHMENT_BYTES);
        let mut payload = mk_payload("hi", 0, false);
        payload.pasted_images_base64 = (0..5)
            .map(|i| STANDARD.encode(vec![i as u8; chunk]))
            .collect();

        let err = run_attachment_pipeline(&store, "fb-aggregate", &payload).unwrap_err();
        assert!(err.contains("exceeds"));
        assert!(err.contains(&MAX_TOTAL_ATTACHMENT_BYTES.to_string()));
    }

    #[test]
    fn submit_feedback_pipeline_succeeds_for_normal_sized_attachments() {
        let (_temp, app) = state_with_tmp();
        let store = app.store();

        let payload = mk_payload("hello world", 2, true);
        let (screenshot, pasted, voice) =
            run_attachment_pipeline(&store, "fb-normal", &payload).unwrap();

        assert_eq!(screenshot.as_deref(), Some("canvas.png"));
        assert_eq!(pasted.len(), 2);
        assert!(voice.is_none());
        assert!(store
            .read_attachment("fb-normal", "canvas.png")
            .unwrap()
            .is_some());
    }

    #[test]
    fn storyboard_screens_persist_as_separate_ordered_attachments() {
        let (_temp, app) = state_with_tmp();
        let store = app.store();
        let png = STANDARD.encode([137u8, 80, 78, 71, 13, 10, 26, 10]);
        let mut payload = mk_payload("two-screen app", 0, false);
        payload.screens = vec![
            FeedbackScreenPayload {
                id: "dashboard".into(),
                name: "Dashboard".into(),
                description: "Overview".into(),
                intended_route: "/".into(),
                annotations: vec![serde_json::json!({"type": "rect"})],
                pasted_images_base64: vec![],
                screenshot_base64: Some(png.clone()),
                window_width: 1200,
                window_height: 800,
            },
            FeedbackScreenPayload {
                id: "settings".into(),
                name: "Settings".into(),
                description: "Account preferences".into(),
                intended_route: "/settings".into(),
                annotations: vec![],
                pasted_images_base64: vec![png.clone()],
                screenshot_base64: Some(png),
                window_width: 1200,
                window_height: 800,
            },
        ];

        let mut budget = AttachmentBudget::new();
        let records =
            persist_storyboard_screens(&store, "fb-storyboard", &payload, &mut budget).unwrap();

        assert_eq!(records.len(), 2);
        assert_eq!(records[0].name, "Dashboard");
        assert_eq!(records[1].intended_route, "/settings");
        assert_eq!(
            records[0].screenshot_filename.as_deref(),
            Some("screen-01.png")
        );
        assert_eq!(records[1].pasted_images, vec!["screen-02-paste-0.png"]);
        assert!(store
            .read_attachment("fb-storyboard", "screen-02.png")
            .unwrap()
            .is_some());
    }

    #[test]
    fn editable_screen_restores_attachment_data_urls_without_host_paths() {
        let (_temp, app) = state_with_tmp();
        let store = app.store();
        let png = [137u8, 80, 78, 71, 13, 10, 26, 10];
        store
            .save_attachment("fb-edit", "paste-0.png", &png)
            .unwrap();

        let editable = editable_screen(
            &store,
            "fb-edit",
            "screen-1",
            "Home",
            "Landing page",
            "/",
            &[serde_json::json!({
                "type": "image", "x": 0.1, "y": 0.2, "w": 0.3, "h": 0.4,
                "assetIndex": 0
            })],
            &["paste-0.png".into()],
            1200,
            800,
        )
        .unwrap();

        assert_eq!(editable.name, "Home");
        assert_eq!(editable.pasted_image_data_urls.len(), 1);
        assert!(editable.pasted_image_data_urls[0].starts_with("data:image/png;base64,"));
        assert!(!editable.pasted_image_data_urls[0].contains("fb-edit"));
    }

    #[test]
    fn submit_feedback_stores_record_and_spawns_job() {
        let (_temp, app) = state_with_tmp();
        let store = app.store();

        let payload = mk_payload("hello world", 2, true);
        // We call the handler body directly since `tauri::command` macros
        // expand to shims; the core store logic is what we verify.
        let before_len = store.list_feedback().unwrap().len();
        let record = {
            let now = current_time_unix_ms();
            let id = format!("fb-{now}-manual");
            let mut rec = FeedbackRecord {
                id: id.clone(),
                feedback_type: payload.feedback_type,
                status: FeedbackStatus::New,
                page_route: payload.page_route.clone(),
                feedback_text: payload.feedback_text.clone(),
                annotations: payload.annotations.clone(),
                pasted_images: vec![],
                screenshot_filename: None,
                screens: vec![],
                voice_filename: None,
                voice_transcript: None,
                window_width: payload.window_width,
                window_height: payload.window_height,
                created_at_unix_ms: now,
                updated_at_unix_ms: now,
                lineage_job_id: None,
                local_delegate: None,
                revision: Default::default(),
            };
            store.save_feedback(&rec).unwrap();
            let engine = LineageEngine::new(&store);
            engine
                .enqueue_job_for_feedback(&mut rec, AgentKind::default())
                .unwrap();
            rec
        };

        assert!(record.lineage_job_id.is_some());
        assert_eq!(store.list_feedback().unwrap().len(), before_len + 1);
        assert_eq!(store.list_lineage_jobs().unwrap().len(), 1);
        let revision = next_feedback_revision(&record);
        assert_eq!(revision.number, 2);
        assert_eq!(
            revision.parent_feedback_id.as_deref(),
            Some(record.id.as_str())
        );
        assert!(revision.superseded_by_feedback_id.is_none());
    }

    #[test]
    fn guess_voice_ext_maps_known_types() {
        assert_eq!(guess_voice_ext(Some("audio/webm")), "webm");
        assert_eq!(guess_voice_ext(Some("audio/ogg;codecs=opus")), "ogg");
        assert_eq!(guess_voice_ext(None), "bin");
    }

    #[test]
    fn submit_feedback_persists_explicit_agent_on_lineage_job() {
        let (_temp, app) = state_with_tmp();
        let store = app.store();

        // Directly exercise the enqueue path with a non-default agent and
        // assert the record round-trips with the right AgentKind. We avoid
        // the full `submit_feedback` shim here because it needs a tauri
        // `State` handle; the engine call exercises the meaningful plumbing.
        let mut rec = FeedbackRecord {
            id: "fb-agent".into(),
            feedback_type: FeedbackType::Bug,
            status: FeedbackStatus::New,
            page_route: "/".into(),
            feedback_text: "use codex please".into(),
            annotations: vec![],
            pasted_images: vec![],
            screenshot_filename: None,
            screens: vec![],
            voice_filename: None,
            voice_transcript: None,
            window_width: 100,
            window_height: 100,
            created_at_unix_ms: 0,
            updated_at_unix_ms: 0,
            lineage_job_id: None,
            local_delegate: None,
            revision: Default::default(),
        };
        store.save_feedback(&rec).unwrap();
        let engine = LineageEngine::new(&store);
        let job = engine
            .enqueue_job_for_feedback(&mut rec, AgentKind::CodexCli)
            .unwrap();
        assert_eq!(job.agent, AgentKind::CodexCli);
        let loaded = store.load_lineage_job(&job.id).unwrap().unwrap();
        assert_eq!(loaded.agent, AgentKind::CodexCli);
    }

    #[test]
    fn list_available_agents_returns_every_variant() {
        let all = list_available_agents().unwrap();
        assert_eq!(all.len(), AgentKind::all().len());
        for kind in AgentKind::all() {
            assert!(all.iter().any(|a| a.kind == kind));
        }
    }

    fn seed_pending_job(store: &crate::store::Store) -> LineageJobRecord {
        let mut rec = FeedbackRecord {
            id: "fb-preview".into(),
            feedback_type: FeedbackType::FeatureRequest,
            status: FeedbackStatus::New,
            page_route: "/".into(),
            feedback_text: "add a budget tracker".into(),
            annotations: vec![],
            pasted_images: vec![],
            screenshot_filename: None,
            screens: vec![],
            voice_filename: None,
            voice_transcript: None,
            window_width: 800,
            window_height: 600,
            created_at_unix_ms: 0,
            updated_at_unix_ms: 0,
            lineage_job_id: None,
            local_delegate: None,
            revision: Default::default(),
        };
        store.save_feedback(&rec).unwrap();
        let engine = LineageEngine::new(store);
        engine
            .enqueue_job_for_feedback(&mut rec, AgentKind::ClaudeCode)
            .unwrap()
    }

    #[test]
    fn preview_returns_summary_with_no_plan_recorded_when_plan_json_absent() {
        let (_temp, app) = state_with_tmp();
        let store = app.store();
        let job = seed_pending_job(&store);

        let preview = preview_lineage_evolution_impl(store.clone(), &job.id).unwrap();
        assert_eq!(preview.job_id, job.id);
        assert_eq!(preview.agent, AgentKind::ClaudeCode);
        assert!(preview.planned_files.is_empty());
        assert!(
            preview.notes.iter().any(|n| n.contains("No plan recorded")),
            "expected a 'no plan recorded' note, got: {:?}",
            preview.notes
        );
        assert!(preview.captured_at_unix_ms > 0);
    }

    #[test]
    fn preview_summarises_plan_json_when_present() {
        use crate::plan::{
            save_plan, AppIdentity, CommandPlan, ComponentPlan, IterationPlan, PlanStage,
            PLAN_SCHEMA_VERSION,
        };
        let (_temp, app) = state_with_tmp();
        let store = app.store();
        let job = seed_pending_job(&store);

        let workspace_root = store.layout().root().to_path_buf();
        let job_dir = runner::job_workspace_dir(&workspace_root, &job.id);
        let mut plan = IterationPlan {
            schema_version: PLAN_SCHEMA_VERSION,
            app: AppIdentity {
                name: "Budgeter".into(),
                one_liner: "Track monthly spend".into(),
                job_id: job.id.clone(),
                iteration: job.iteration,
                ..Default::default()
            },
            stage: PlanStage::BackendPlanned,
            ..Default::default()
        };
        plan.backend.commands.push(CommandPlan {
            name: "create_budget".into(),
            input: "CreateBudgetPayload".into(),
            output: "Budget".into(),
            motivated_by_regions: vec!["R1".into()],
            summary: "Create a budget".into(),
            body_sketch: "Generate id, build Budget, persist via store.save_budget(rec).".into(),
            anchors: vec!["save_budget".into()],
        });
        plan.frontend.components.push(ComponentPlan {
            name: "BudgetForm".into(),
            module: "budget_form.rs".into(),
            uses_commands: vec!["create_budget".into()],
            summary: "Budget create form".into(),
            motivated_by_regions: vec!["R1".into()],
        });
        save_plan(&job_dir, &plan).unwrap();

        let preview = preview_lineage_evolution_impl(store, &job.id).unwrap();
        assert!(preview.plan_summary.contains("Budgeter"));
        assert!(preview
            .planned_files
            .iter()
            .any(|f| f.contains("create_budget")));
        assert!(preview
            .planned_files
            .iter()
            .any(|f| f.contains("BudgetForm")));
    }

    #[test]
    fn preview_errors_for_unknown_job_id() {
        let (_temp, app) = state_with_tmp();
        let store = app.store();
        let err = preview_lineage_evolution_impl(store, "nonexistent-job").unwrap_err();
        assert!(err.contains("not found"));
    }

    #[test]
    fn preview_summary_tolerates_extra_fields() {
        // Forward-compat: the UI should keep working if a future host adds
        // fields to PreviewSummary.
        let raw = r#"{
            "jobId": "job-x",
            "agent": "claude_code",
            "sourceIteration": 0,
            "targetIteration": 1,
            "targetPort": 1531,
            "planSummary": "hello",
            "plannedFiles": ["command: foo"],
            "notes": [],
            "capturedAtUnixMs": 1,
            "futureField": 42
        }"#;
        let p: PreviewSummary = serde_json::from_str(raw).unwrap();
        assert_eq!(p.job_id, "job-x");
        assert_eq!(p.target_port, 1531);
        assert_eq!(p.planned_files.len(), 1);
    }
}
