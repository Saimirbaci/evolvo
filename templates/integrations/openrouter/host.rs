//! Tauri commands for the OpenRouter LLM integration.
//!
//! Register every `#[tauri::command]` below in your `invoke_handler`. Wire
//! `AppState` so the commands can resolve the workspace root (reused for
//! session persistence).

use std::path::PathBuf;

use keyring::Entry;
use serde::{Deserialize, Serialize};

use super::session::{
    build_system_prompt, now_ms, ChatSession, Message, Role, SessionStore, SessionSummary,
    DEFAULT_MODEL,
};

const KEYRING_SERVICE: &str = "evolvo.openrouter";
const KEYRING_ACCOUNT: &str = "api_key";
const API_BASE: &str = "https://openrouter.ai/api/v1";

fn key_entry() -> Result<Entry, String> {
    Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT).map_err(|e| format!("keyring init: {e}"))
}

fn load_api_key() -> Result<String, String> {
    key_entry()?
        .get_password()
        .map_err(|e| format!("no OpenRouter API key stored ({e}) — ask the user to set one"))
}

fn workspace_root(state: &tauri::State<'_, crate::AppState>) -> PathBuf {
    // Reuse whatever your AppState exposes. The host Store knows its layout.
    state.store.layout().root().to_path_buf()
}

fn session_store(state: &tauri::State<'_, crate::AppState>) -> SessionStore {
    SessionStore::new(&workspace_root(state))
}

// ───────── Secret commands ─────────────────────────────────────────────────

#[tauri::command]
pub async fn openrouter_set_api_key(key: String) -> Result<(), String> {
    if key.trim().is_empty() {
        return Err("API key is empty".into());
    }
    key_entry()?
        .set_password(key.trim())
        .map_err(|e| format!("keyring set: {e}"))
}

#[tauri::command]
pub async fn openrouter_has_api_key() -> Result<bool, String> {
    Ok(key_entry()?.get_password().is_ok())
}

#[tauri::command]
pub async fn openrouter_clear_api_key() -> Result<(), String> {
    // Ignore "not found" errors — idempotent delete.
    let _ = key_entry()?.delete_credential();
    Ok(())
}

// ───────── Session commands ────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionArgs {
    pub app_name: String,
    pub model: Option<String>,
}

#[tauri::command]
pub async fn openrouter_create_session(
    args: CreateSessionArgs,
    state: tauri::State<'_, crate::AppState>,
) -> Result<ChatSession, String> {
    session_store(&state)
        .create(args.app_name, args.model)
        .map_err(|e| format!("create session: {e}"))
}

#[tauri::command]
pub async fn openrouter_list_sessions(
    state: tauri::State<'_, crate::AppState>,
) -> Result<Vec<SessionSummary>, String> {
    session_store(&state)
        .list()
        .map_err(|e| format!("list sessions: {e}"))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionIdArgs {
    pub session_id: String,
}

#[tauri::command]
pub async fn openrouter_load_session(
    args: SessionIdArgs,
    state: tauri::State<'_, crate::AppState>,
) -> Result<ChatSession, String> {
    session_store(&state)
        .load(&args.session_id)
        .map_err(|e| format!("load session: {e}"))
}

#[tauri::command]
pub async fn openrouter_delete_session(
    args: SessionIdArgs,
    state: tauri::State<'_, crate::AppState>,
) -> Result<(), String> {
    session_store(&state)
        .delete(&args.session_id)
        .map_err(|e| format!("delete session: {e}"))
}

// ───────── Chat completion ─────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendMessageArgs {
    pub session_id: String,
    pub user_message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendMessageResult {
    pub session: ChatSession,
    pub assistant_message: Message,
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    #[serde(default)]
    pub prompt_tokens: u32,
    #[serde(default)]
    pub completion_tokens: u32,
    #[serde(default)]
    pub total_tokens: u32,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatRequestMessage<'a>>,
}

#[derive(Serialize)]
struct ChatRequestMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
    #[serde(default)]
    usage: Option<Usage>,
    #[serde(default)]
    error: Option<ApiError>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatResponseMessage,
}

#[derive(Deserialize)]
struct ChatResponseMessage {
    content: String,
}

#[derive(Deserialize)]
struct ApiError {
    message: String,
    #[serde(default)]
    code: Option<serde_json::Value>,
}

fn role_str(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::System => "system",
    }
}

#[tauri::command]
pub async fn openrouter_send_message(
    args: SendMessageArgs,
    state: tauri::State<'_, crate::AppState>,
) -> Result<SendMessageResult, String> {
    let api_key = load_api_key()?;
    let store = session_store(&state);

    // 1. Append the user message first so a failed request still preserves
    //    what the user typed.
    let user_msg = Message {
        role: Role::User,
        content: args.user_message.clone(),
        created_at_ms: now_ms(),
    };
    let session = store
        .append(&args.session_id, user_msg)
        .map_err(|e| format!("append user message: {e}"))?;

    // 2. Rebuild the wire message list: fresh system prompt + persisted history.
    let system = build_system_prompt(&session.app_name);
    let mut wire: Vec<ChatRequestMessage> = Vec::with_capacity(session.messages.len() + 1);
    wire.push(ChatRequestMessage {
        role: "system",
        content: &system,
    });
    for m in &session.messages {
        wire.push(ChatRequestMessage {
            role: role_str(m.role),
            content: &m.content,
        });
    }

    let model = if session.model.is_empty() {
        DEFAULT_MODEL
    } else {
        session.model.as_str()
    };

    let body = ChatRequest {
        model,
        messages: wire,
    };

    // 3. POST /chat/completions.
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| format!("http client: {e}"))?;

    let res = client
        .post(format!("{API_BASE}/chat/completions"))
        .bearer_auth(&api_key)
        .header("HTTP-Referer", "https://evolvo.local")
        .header("X-Title", &session.app_name)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("openrouter request: {e}"))?;

    let status = res.status();
    let payload: ChatResponse = res
        .json()
        .await
        .map_err(|e| format!("openrouter decode ({status}): {e}"))?;

    if let Some(err) = payload.error {
        return Err(format!("openrouter error: {}", err.message));
    }
    let choice = payload
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| "openrouter returned no choices".to_string())?;

    // 4. Persist the assistant reply on top of history.
    let assistant_msg = Message {
        role: Role::Assistant,
        content: choice.message.content,
        created_at_ms: now_ms(),
    };
    let session = store
        .append(&args.session_id, assistant_msg.clone())
        .map_err(|e| format!("append assistant message: {e}"))?;

    Ok(SendMessageResult {
        session,
        assistant_message: assistant_msg,
        usage: payload.usage,
    })
}

// ───────── Model catalogue ─────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub context_length: Option<u32>,
}

#[derive(Deserialize)]
struct ModelsResponse {
    data: Vec<ModelInfo>,
}

#[tauri::command]
pub async fn openrouter_list_models() -> Result<Vec<ModelInfo>, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let res = client
        .get(format!("{API_BASE}/models"))
        .send()
        .await
        .map_err(|e| format!("openrouter models: {e}"))?;
    let parsed: ModelsResponse = res
        .json()
        .await
        .map_err(|e| format!("decode models: {e}"))?;
    Ok(parsed.data)
}

#[cfg(test)]
mod tests {
    use super::super::session::{build_system_prompt, SessionStore};
    use tempfile::tempdir;

    #[test]
    fn system_prompt_mentions_app_name() {
        let s = build_system_prompt("CrayonCRM");
        assert!(s.contains("CrayonCRM"));
    }

    #[test]
    fn session_store_round_trip() {
        let dir = tempdir().unwrap();
        let store = SessionStore::new(dir.path());
        let s = store.create("T".into(), None).unwrap();
        let loaded = store.load(&s.id).unwrap();
        assert_eq!(loaded.id, s.id);
    }
}
