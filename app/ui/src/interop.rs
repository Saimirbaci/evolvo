use js_sys::{Function, Promise, Reflect};
use serde::de::DeserializeOwned;
use serde::Serialize;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::window;

use crate::types::{
    AgentAvailability, AgentKind, AnswerPlanQuestionsPayload, AppHealth, DelegationSummary,
    DeleteSummary, DesignSummary, DesktopSession, EditableDesign, EditableFeedback, FeedbackRecord,
    JobActivity, LineageArtifactsChanged, LineageJobRecord, LineageJobUpserted,
    LocalDelegateAvailability, PlanAnswerPayload, PreviewSummary, SaveDesignPayload, SketchRequest,
    SketchResponse, StageState, SubmitFeedbackPayload, WindowCapture,
};

pub const LINEAGE_JOB_UPSERTED_EVENT: &str = "lineage://job-upserted";
pub const LINEAGE_ARTIFACTS_CHANGED_EVENT: &str = "lineage://artifacts-changed";
pub const LINEAGE_SNAPSHOT_INVALIDATED_EVENT: &str = "lineage://snapshot-invalidated";
/// Emitted by the host to the desktop overlay window when a fresh capture
/// is ready (payload: capture timestamp). See `desktop.rs`.
pub const DESKTOP_CAPTURE_EVENT: &str = "evolvo://desktop-capture";

fn js_error(v: JsValue) -> String {
    v.as_string()
        .unwrap_or_else(|| "tauri invocation failed".into())
}

/// Owns both sides of a Tauri JS event subscription. Dropping it calls the
/// unlisten function before releasing the WASM callback, preventing duplicate
/// listeners when the Lineage route is mounted more than once.
pub struct TauriEventListener {
    unlisten: Function,
    _callback: wasm_bindgen::closure::Closure<dyn FnMut(JsValue)>,
}

impl Drop for TauriEventListener {
    fn drop(&mut self) {
        let _ = self.unlisten.call0(&JsValue::NULL);
    }
}

async fn listen_event<T, F>(event_name: &str, mut handler: F) -> Result<TauriEventListener, String>
where
    T: DeserializeOwned + 'static,
    F: FnMut(T) + 'static,
{
    let win = window().ok_or_else(|| "window is not available".to_string())?;
    let tauri = Reflect::get(&win, &JsValue::from_str("__TAURI__")).map_err(js_error)?;
    let event_api = Reflect::get(&tauri, &JsValue::from_str("event")).map_err(js_error)?;
    let listen = Reflect::get(&event_api, &JsValue::from_str("listen")).map_err(js_error)?;
    let listen: Function = listen
        .dyn_into()
        .map_err(|_| "window.__TAURI__.event.listen is unavailable".to_string())?;

    let callback = wasm_bindgen::closure::Closure::<dyn FnMut(JsValue)>::new(move |event| {
        let Ok(payload) = Reflect::get(&event, &JsValue::from_str("payload")) else {
            return;
        };
        if let Ok(value) = serde_wasm_bindgen::from_value::<T>(payload) {
            handler(value);
        }
    });
    let promise = listen
        .call2(
            &event_api,
            &JsValue::from_str(event_name),
            callback.as_ref().unchecked_ref(),
        )
        .map_err(js_error)?;
    let promise: Promise = promise
        .dyn_into()
        .map_err(|_| "Tauri event.listen did not return a Promise".to_string())?;
    let unlisten = JsFuture::from(promise).await.map_err(js_error)?;
    let unlisten: Function = unlisten
        .dyn_into()
        .map_err(|_| "Tauri event.listen did not return an unlisten function".to_string())?;
    Ok(TauriEventListener {
        unlisten,
        _callback: callback,
    })
}

pub async fn listen_lineage_job_upserted(
    handler: impl FnMut(LineageJobUpserted) + 'static,
) -> Result<TauriEventListener, String> {
    listen_event(LINEAGE_JOB_UPSERTED_EVENT, handler).await
}

pub async fn listen_lineage_artifacts_changed(
    handler: impl FnMut(LineageArtifactsChanged) + 'static,
) -> Result<TauriEventListener, String> {
    listen_event(LINEAGE_ARTIFACTS_CHANGED_EVENT, handler).await
}

pub async fn listen_lineage_snapshot_invalidated(
    handler: impl FnMut(serde_json::Value) + 'static,
) -> Result<TauriEventListener, String> {
    listen_event(LINEAGE_SNAPSHOT_INVALIDATED_EVENT, handler).await
}

pub async fn invoke_command<T: DeserializeOwned>(command: &str) -> Result<T, String> {
    let win = window().ok_or_else(|| "window is not available".to_string())?;
    let tauri = Reflect::get(&win, &JsValue::from_str("__TAURI__")).map_err(js_error)?;
    let core = Reflect::get(&tauri, &JsValue::from_str("core")).map_err(js_error)?;
    let invoke = Reflect::get(&core, &JsValue::from_str("invoke")).map_err(js_error)?;
    let invoke: Function = invoke
        .dyn_into()
        .map_err(|_| "window.__TAURI__.core.invoke is unavailable".to_string())?;
    let promise = invoke
        .call1(&core, &JsValue::from_str(command))
        .map_err(js_error)?;
    let promise: Promise = promise
        .dyn_into()
        .map_err(|_| "invoke did not return a Promise".to_string())?;
    let value = JsFuture::from(promise).await.map_err(js_error)?;
    serde_wasm_bindgen::from_value(value).map_err(|e| e.to_string())
}

pub async fn invoke_command_with_args<T, A>(command: &str, args: &A) -> Result<T, String>
where
    T: DeserializeOwned,
    A: Serialize,
{
    #[derive(Serialize)]
    struct Wrapper<'a, A: Serialize> {
        payload: &'a A,
    }

    let win = window().ok_or_else(|| "window is not available".to_string())?;
    let tauri = Reflect::get(&win, &JsValue::from_str("__TAURI__")).map_err(js_error)?;
    let core = Reflect::get(&tauri, &JsValue::from_str("core")).map_err(js_error)?;
    let invoke = Reflect::get(&core, &JsValue::from_str("invoke")).map_err(js_error)?;
    let invoke: Function = invoke
        .dyn_into()
        .map_err(|_| "window.__TAURI__.core.invoke is unavailable".to_string())?;

    let wrapped = Wrapper { payload: args };
    let args_js = serde_wasm_bindgen::to_value(&wrapped).map_err(|e| e.to_string())?;

    let promise = invoke
        .call2(&core, &JsValue::from_str(command), &args_js)
        .map_err(js_error)?;
    let promise: Promise = promise
        .dyn_into()
        .map_err(|_| "invoke did not return a Promise".to_string())?;
    let value = JsFuture::from(promise).await.map_err(js_error)?;
    serde_wasm_bindgen::from_value(value).map_err(|e| e.to_string())
}

#[derive(Serialize)]
struct IdArg<'a> {
    id: &'a str,
}

pub async fn app_health() -> Result<AppHealth, String> {
    invoke_command("app_health").await
}

pub async fn submit_feedback(payload: &SubmitFeedbackPayload) -> Result<FeedbackRecord, String> {
    invoke_command_with_args("submit_feedback", payload).await
}

pub async fn list_feedback() -> Result<Vec<FeedbackRecord>, String> {
    invoke_command("list_feedback").await
}

pub async fn load_feedback(id: &str) -> Result<Option<FeedbackRecord>, String> {
    invoke_command_with_args("load_feedback", &IdArg { id }).await
}

pub async fn load_feedback_for_revision(id: &str) -> Result<Option<EditableFeedback>, String> {
    invoke_command_with_args("load_feedback_for_revision", &IdArg { id }).await
}

pub async fn list_lineage_jobs() -> Result<Vec<LineageJobRecord>, String> {
    invoke_command("list_lineage_jobs").await
}

pub async fn approve_lineage_job(id: &str) -> Result<LineageJobRecord, String> {
    invoke_command_with_args("approve_lineage_job", &IdArg { id }).await
}

/// Record answers to a paused plan's open questions; `replan` re-runs the
/// planner with them ("Answer & re-plan").
pub async fn answer_plan_questions(
    id: &str,
    answers: Vec<PlanAnswerPayload>,
    replan: bool,
) -> Result<LineageJobRecord, String> {
    invoke_command_with_args(
        "answer_plan_questions",
        &AnswerPlanQuestionsPayload {
            id: id.to_string(),
            answers,
            replan,
        },
    )
    .await
}

pub async fn reject_lineage_job(id: &str) -> Result<LineageJobRecord, String> {
    invoke_command_with_args("reject_lineage_job", &IdArg { id }).await
}

pub async fn retry_lineage_job(id: &str) -> Result<LineageJobRecord, String> {
    invoke_command_with_args("retry_lineage_job", &IdArg { id }).await
}

pub async fn run_lineage_job(id: &str) -> Result<LineageJobRecord, String> {
    invoke_command_with_args("run_lineage_job", &IdArg { id }).await
}

#[derive(Serialize)]
struct NoteArgs<'a> {
    id: &'a str,
    note: &'a str,
}

pub async fn append_lineage_note(id: &str, note: &str) -> Result<LineageJobRecord, String> {
    invoke_command_with_args("append_lineage_note", &NoteArgs { id, note }).await
}

#[derive(Serialize)]
struct UrlArg<'a> {
    url: &'a str,
}

pub async fn open_external_url(url: &str) -> Result<(), String> {
    invoke_command_with_args("open_external_url", &UrlArg { url }).await
}

/// Returns base64-encoded PNG of the current Tauri window. Used by the
/// feedback panel to composite annotations onto the real page view, since
/// the canvas overlay itself is transparent and would otherwise leave the
/// submitted screenshot without any page context.
pub async fn save_design(payload: &SaveDesignPayload) -> Result<DesignSummary, String> {
    invoke_command_with_args("save_design", payload).await
}

pub async fn save_design_draft(payload: &SaveDesignPayload) -> Result<DesignSummary, String> {
    invoke_command_with_args("save_design_draft", payload).await
}

pub async fn list_designs() -> Result<Vec<DesignSummary>, String> {
    invoke_command("list_designs").await
}

pub async fn load_design(id: &str) -> Result<Option<EditableDesign>, String> {
    invoke_command_with_args("load_design", &IdArg { id }).await
}

pub async fn load_design_draft() -> Result<Option<EditableDesign>, String> {
    invoke_command("load_design_draft").await
}

pub async fn delete_design(id: &str) -> Result<bool, String> {
    invoke_command_with_args("delete_design", &IdArg { id }).await
}

pub async fn clear_design_draft() -> Result<bool, String> {
    invoke_command("clear_design_draft").await
}

pub async fn capture_window_png() -> Result<String, String> {
    invoke_command("capture_window_png").await
}

/// Frozen desktop frame + target app for the overlay, if one is pending.
pub async fn take_desktop_capture() -> Result<Option<DesktopSession>, String> {
    invoke_command("take_desktop_capture").await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CaptureDesktopWindowPayload {
    id: u32,
}

/// Window mode: capture one window (by OS window id) on its own.
pub async fn capture_desktop_window(id: u32) -> Result<WindowCapture, String> {
    invoke_command_with_args(
        "capture_desktop_window",
        &CaptureDesktopWindowPayload { id },
    )
    .await
}

/// Delete a lineage job and everything under it (worktree, branch, job
/// workspace, its feedback + attachments, the record).
pub async fn delete_lineage_job(id: &str) -> Result<DeleteSummary, String> {
    invoke_command_with_args("delete_lineage_job", &IdArg { id }).await
}

/// Stop the new version Evolvo launched for a job. `true` if one was running.
pub async fn stop_lineage_run(id: &str) -> Result<bool, String> {
    invoke_command_with_args("stop_lineage_run", &IdArg { id }).await
}

/// Live activity (phase, recent agent actions, changed files) for a job.
pub async fn read_job_activity(id: &str) -> Result<JobActivity, String> {
    invoke_command_with_args("read_job_activity", &IdArg { id }).await
}

/// Hide the desktop overlay window and hand focus back to the captured app.
pub async fn close_desktop_overlay() -> Result<(), String> {
    invoke_command("close_desktop_overlay").await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ResolveTargetRepoPayload<'a> {
    id: &'a str,
    repo: Option<&'a str>,
}

/// (Re-)resolve an external-app job's source repo, optionally from a
/// user-supplied local path or https clone URL.
pub async fn resolve_target_repo(
    job_id: &str,
    repo: Option<&str>,
) -> Result<LineageJobRecord, String> {
    invoke_command_with_args(
        "resolve_target_repo",
        &ResolveTargetRepoPayload { id: job_id, repo },
    )
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OpenDesktopRevisionPayload<'a> {
    feedback_id: &'a str,
    agent: AgentKind,
}

/// "Revise on Canvas" for desktop feedback: reopen the desktop overlay with
/// the original frozen screen behind the restored annotations.
pub async fn open_desktop_revision(feedback_id: &str, agent: AgentKind) -> Result<(), String> {
    invoke_command_with_args(
        "open_desktop_revision",
        &OpenDesktopRevisionPayload { feedback_id, agent },
    )
    .await
}

pub async fn listen_desktop_capture(
    handler: impl FnMut(u64) + 'static,
) -> Result<TauriEventListener, String> {
    listen_event(DESKTOP_CAPTURE_EVENT, handler).await
}

/// Experimental: describe a sketch (or a change to the current one) and get
/// back add/update/delete ops for the canvas.
pub async fn generate_sketch(request: &SketchRequest) -> Result<SketchResponse, String> {
    invoke_command_with_args("generate_sketch", request).await
}

pub async fn list_job_stages(id: &str) -> Result<Vec<StageState>, String> {
    invoke_command_with_args("list_job_stages", &IdArg { id }).await
}

pub async fn read_job_plan(id: &str) -> Result<Option<serde_json::Value>, String> {
    invoke_command_with_args("read_job_plan", &IdArg { id }).await
}

#[derive(Serialize)]
struct TailStageLogArgs<'a> {
    id: &'a str,
    stage: &'a str,
    #[serde(rename = "maxBytes")]
    max_bytes: Option<usize>,
}

pub async fn tail_stage_log(
    id: &str,
    stage: &str,
    max_bytes: Option<usize>,
) -> Result<String, String> {
    invoke_command_with_args(
        "tail_stage_log",
        &TailStageLogArgs {
            id,
            stage,
            max_bytes,
        },
    )
    .await
}

/// Re-enter the multi-stage pipeline for a failed/interrupted job.
/// Reuses the existing worktree and plan.json; stages already persisted
/// in `plan.stage` are re-validated without re-invoking Claude.
pub async fn resume_lineage_job(id: &str) -> Result<LineageJobRecord, String> {
    invoke_command_with_args("resume_lineage_job", &IdArg { id }).await
}

/// Enumerate the available agent backends with per-binary install status.
/// The UI uses this to grey out chips for agents the user hasn't
/// installed. Cached server-side so repeated polls are cheap.
pub async fn list_available_agents() -> Result<Vec<AgentAvailability>, String> {
    invoke_command("list_available_agents").await
}

/// Whether local-model delegation (pi + Ollama) is wireable on this
/// host. The feedback panel uses this to gate the "Local worker" chip.
pub async fn list_local_delegate_availability() -> Result<LocalDelegateAvailability, String> {
    invoke_command("list_local_delegate_availability").await
}

/// Aggregated delegation telemetry for one lineage job, parsed from
/// `<job_dir>/pi-events.jsonl`. `Ok(None)` means the job didn't run any
/// delegations — the UI should hide the section instead of rendering
/// zeros. The Lineage detail panel calls this on mount and on every
/// reload tick.
pub async fn read_delegation_summary(id: &str) -> Result<Option<DelegationSummary>, String> {
    invoke_command_with_args("read_delegation_summary", &IdArg { id }).await
}

/// Read-only dry-run preview of what an `Evolve` click on `id` will
/// dispatch. Surfaced in the EvolveConfirmation modal before we
/// actually call `approve_lineage_job` so the reviewer can confirm
/// the planned scope. The host command never mutates state, so this
/// can be called freely (e.g. on every modal open) without side
/// effects.
pub async fn preview_lineage_evolution(id: &str) -> Result<PreviewSummary, String> {
    invoke_command_with_args("preview_lineage_evolution", &IdArg { id }).await
}

#[derive(Serialize)]
struct ExportLineageArgs<'a> {
    id: &'a str,
    #[serde(rename = "destinationDir", skip_serializing_if = "Option::is_none")]
    destination_dir: Option<&'a str>,
}

#[derive(serde::Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ExportLineageResult {
    pub bundle_path: String,
}

/// Operationalises product invariant I-P4: write a portable `.evolvo-bundle`
/// for the given lineage job. Pass `destination_dir = None` to land in the
/// workspace's `exports/` directory.
pub async fn export_lineage(
    id: &str,
    destination_dir: Option<&str>,
) -> Result<ExportLineageResult, String> {
    invoke_command_with_args(
        "export_lineage",
        &ExportLineageArgs {
            id,
            destination_dir,
        },
    )
    .await
}

#[derive(Serialize)]
struct ImportLineageArgs<'a> {
    #[serde(rename = "bundlePath")]
    bundle_path: &'a str,
    #[serde(rename = "targetWorkspaceRoot")]
    target_workspace_root: &'a str,
}

#[derive(serde::Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummary {
    pub workspace_root: String,
    pub primary_job_id: String,
    pub feedback_count: u32,
    pub attachment_count: u32,
}

pub async fn import_lineage_bundle(
    bundle_path: &str,
    target_workspace_root: &str,
) -> Result<ImportSummary, String> {
    invoke_command_with_args(
        "import_lineage_bundle",
        &ImportLineageArgs {
            bundle_path,
            target_workspace_root,
        },
    )
    .await
}
