use js_sys::{Function, Promise, Reflect};
use serde::de::DeserializeOwned;
use serde::Serialize;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::window;

use crate::types::{AppHealth, FeedbackRecord, LineageJobRecord, StageState, SubmitFeedbackPayload};

fn js_error(v: JsValue) -> String {
    v.as_string()
        .unwrap_or_else(|| "tauri invocation failed".into())
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

pub async fn list_lineage_jobs() -> Result<Vec<LineageJobRecord>, String> {
    invoke_command("list_lineage_jobs").await
}

pub async fn approve_lineage_job(id: &str) -> Result<LineageJobRecord, String> {
    invoke_command_with_args("approve_lineage_job", &IdArg { id }).await
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
pub async fn capture_window_png() -> Result<String, String> {
    invoke_command("capture_window_png").await
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

pub async fn tail_stage_log(id: &str, stage: &str, max_bytes: Option<usize>) -> Result<String, String> {
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
