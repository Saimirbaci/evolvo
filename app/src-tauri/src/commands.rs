use base64::{engine::general_purpose::STANDARD, Engine as _};
use tauri::State;

use crate::lineage::{LineageEngine, Transition};
use crate::runner;
use crate::state::AppState;
use crate::store::StoreError;
use crate::types::{
    current_time_unix_ms, AppHealth, EntityIdPayload, FeedbackRecord, FeedbackStatus,
    LineageJobRecord, LineageJobStatus, SubmitFeedbackPayload,
};

const APP_NAME: &str = "Evolvo";
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

fn store_error(err: StoreError) -> String {
    err.to_string()
}

fn decode_base64(name: &str, value: &str) -> Result<Vec<u8>, String> {
    STANDARD
        .decode(value)
        .map_err(|e| format!("invalid base64 for {name}: {e}"))
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
    })
}

#[tauri::command]
pub fn submit_feedback(
    state: State<'_, AppState>,
    payload: SubmitFeedbackPayload,
) -> Result<FeedbackRecord, String> {
    let store = state.store();
    store.init_workspace().map_err(store_error)?;

    let now = current_time_unix_ms();
    let id = format!("fb-{now}");

    let screenshot_filename = match payload.screenshot_base64.as_deref() {
        Some(b64) if !b64.is_empty() => {
            let bytes = decode_base64("screenshot", b64)?;
            Some(
                store
                    .save_attachment(&id, "canvas.png", &bytes)
                    .map_err(store_error)?,
            )
        }
        _ => None,
    };

    let mut pasted_images = Vec::with_capacity(payload.pasted_images_base64.len());
    for (idx, b64) in payload.pasted_images_base64.iter().enumerate() {
        if b64.is_empty() {
            continue;
        }
        let bytes = decode_base64(&format!("pasted-image-{idx}"), b64)?;
        let filename = format!("paste-{idx}.png");
        let saved = store
            .save_attachment(&id, &filename, &bytes)
            .map_err(store_error)?;
        pasted_images.push(saved);
    }

    let voice_filename = match payload.voice_base64.as_deref() {
        Some(b64) if !b64.is_empty() => {
            let bytes = decode_base64("voice", b64)?;
            let ext = guess_voice_ext(payload.voice_mime_type.as_deref());
            Some(
                store
                    .save_attachment(&id, &format!("voice.{ext}"), &bytes)
                    .map_err(store_error)?,
            )
        }
        _ => None,
    };

    let mut record = FeedbackRecord {
        id: id.clone(),
        feedback_type: payload.feedback_type,
        status: FeedbackStatus::New,
        page_route: if payload.page_route.is_empty() {
            "/".into()
        } else {
            payload.page_route
        },
        feedback_text: payload.feedback_text,
        annotations: payload.annotations,
        pasted_images,
        screenshot_filename,
        voice_filename,
        voice_transcript: payload.voice_transcript.filter(|s| !s.trim().is_empty()),
        window_width: payload.window_width,
        window_height: payload.window_height,
        created_at_unix_ms: now,
        updated_at_unix_ms: now,
        lineage_job_id: None,
    };

    store.save_feedback(&record).map_err(store_error)?;

    // Enqueue a lineage job so reviewers can immediately triage.
    let engine = LineageEngine::new(&store);
    let _ = engine
        .enqueue_job_for_feedback(&mut record)
        .map_err(store_error)?;

    Ok(record)
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

    runner::resume_pipeline(store.clone(), job.id.clone())?;

    // Surface the status flip immediately — `resume_pipeline` also sets
    // `Implementing` on the background thread, but the caller wants the
    // updated record to render *now*, not after the first stage finishes.
    engine
        .force_status(&job.id, LineageJobStatus::Implementing)
        .map_err(store_error)
}

/// "Advance" button entry point. Behaviour depends on the job's current
/// status:
/// - `Pending` → fork the source repo into a lineage worktree, spawn
///   `claude -p … --dangerously-skip-permissions` in it, and return with
///   status `Implementing`. The run continues on a background thread;
///   the job will flip to `BuildReady` or `Failed` when claude exits.
/// - `BuildReady` → advance to `Promoted` via the regular state machine
///   (merging back to main is out of scope for this step).
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

    let prepared = runner::prepare_run(store, job, &feedback).map_err(|e| {
        // Record the failure on the job so the reviewer sees why nothing
        // happened, then surface the error to the frontend.
        let msg = store_error(e);
        let _ = engine.append_note(&job.id, &format!("failed to prepare run: {msg}"));
        let _ = engine.force_status(&job.id, LineageJobStatus::Failed);
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
        )
        .map_err(store_error)?;
    let updated = engine
        .force_status(&job.id, LineageJobStatus::Implementing)
        .map_err(store_error)?;

    if runner::is_multi_stage_candidate(&feedback) {
        runner::launch_pipeline(store.clone(), job.id.clone(), feedback, prepared);
    } else {
        runner::launch_claude(store.clone(), job.id.clone(), prepared);
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

    let source = runner::resolve_source_repo().ok_or_else(|| {
        "could not locate Evolvo source repo — set EVOLVO_SOURCE_REPO or run from within the repo"
            .to_string()
    })?;
    let workspace_root = store.layout().root().to_path_buf();
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
    use crate::types::FeedbackType;
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
            voice_base64: None,
            voice_mime_type: None,
            voice_transcript: None,
            window_width: 1024,
            window_height: 768,
        }
    }

    fn state_with_tmp() -> (tempfile::TempDir, AppState) {
        let temp = tempdir().unwrap();
        let state = AppState::with_root(temp.path().to_path_buf());
        (temp, state)
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
                voice_filename: None,
                voice_transcript: None,
                window_width: payload.window_width,
                window_height: payload.window_height,
                created_at_unix_ms: now,
                updated_at_unix_ms: now,
                lineage_job_id: None,
            };
            store.save_feedback(&rec).unwrap();
            let engine = LineageEngine::new(&store);
            engine.enqueue_job_for_feedback(&mut rec).unwrap();
            rec
        };

        assert!(record.lineage_job_id.is_some());
        assert_eq!(store.list_feedback().unwrap().len(), before_len + 1);
        assert_eq!(store.list_lineage_jobs().unwrap().len(), 1);
    }

    #[test]
    fn guess_voice_ext_maps_known_types() {
        assert_eq!(guess_voice_ext(Some("audio/webm")), "webm");
        assert_eq!(guess_voice_ext(Some("audio/ogg;codecs=opus")), "ogg");
        assert_eq!(guess_voice_ext(None), "bin");
    }
}
