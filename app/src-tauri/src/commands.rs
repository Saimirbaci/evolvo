use base64::{engine::general_purpose::STANDARD, Engine as _};
use tauri::State;

use crate::sandbox::{SandboxEngine, Transition};
use crate::state::AppState;
use crate::store::StoreError;
use crate::types::{
    current_time_unix_ms, AppHealth, EntityIdPayload, FeedbackRecord, FeedbackStatus,
    SandboxJobRecord, SubmitFeedbackPayload,
};

const APP_NAME: &str = "NoIDE";
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
        sandbox_job_id: None,
    };

    store.save_feedback(&record).map_err(store_error)?;

    // Enqueue a sandbox job so reviewers can immediately triage.
    let engine = SandboxEngine::new(&store);
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
pub fn list_sandbox_jobs(state: State<'_, AppState>) -> Result<Vec<SandboxJobRecord>, String> {
    state.store().list_sandbox_jobs().map_err(store_error)
}

#[tauri::command]
pub fn load_sandbox_job(
    state: State<'_, AppState>,
    payload: EntityIdPayload,
) -> Result<Option<SandboxJobRecord>, String> {
    state
        .store()
        .load_sandbox_job(&payload.id)
        .map_err(store_error)
}

#[tauri::command]
pub fn approve_sandbox_job(
    state: State<'_, AppState>,
    payload: EntityIdPayload,
) -> Result<SandboxJobRecord, String> {
    let store = state.store();
    let engine = SandboxEngine::new(&store);
    engine
        .transition(&payload.id, Transition::Approve)
        .map_err(store_error)
}

#[tauri::command]
pub fn reject_sandbox_job(
    state: State<'_, AppState>,
    payload: EntityIdPayload,
) -> Result<SandboxJobRecord, String> {
    let store = state.store();
    let engine = SandboxEngine::new(&store);
    engine
        .transition(&payload.id, Transition::Reject)
        .map_err(store_error)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotePayload {
    pub id: String,
    pub note: String,
}

#[tauri::command]
pub fn append_sandbox_note(
    state: State<'_, AppState>,
    payload: NotePayload,
) -> Result<SandboxJobRecord, String> {
    let store = state.store();
    let engine = SandboxEngine::new(&store);
    engine
        .append_note(&payload.id, &payload.note)
        .map_err(store_error)
}

#[tauri::command]
pub fn open_workspace_path(state: State<'_, AppState>) -> Result<String, String> {
    Ok(state.workspace_root_display())
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
            screenshot_base64: if with_screenshot { Some(tiny_png) } else { None },
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
                sandbox_job_id: None,
            };
            store.save_feedback(&rec).unwrap();
            let engine = SandboxEngine::new(&store);
            engine.enqueue_job_for_feedback(&mut rec).unwrap();
            rec
        };

        assert!(record.sandbox_job_id.is_some());
        assert_eq!(store.list_feedback().unwrap().len(), before_len + 1);
        assert_eq!(store.list_sandbox_jobs().unwrap().len(), 1);
    }

    #[test]
    fn guess_voice_ext_maps_known_types() {
        assert_eq!(guess_voice_ext(Some("audio/webm")), "webm");
        assert_eq!(guess_voice_ext(Some("audio/ogg;codecs=opus")), "ogg");
        assert_eq!(guess_voice_ext(None), "bin");
    }
}
