//! chat_app — host-side scaffold.
//!
//! Drop into `app/src-tauri/src/app_shapes/chat_app.rs`. Register the five
//! `#[tauri::command]` functions in
//! `app/src-tauri/src/lib.rs::run()`'s `invoke_handler!` list.
//!
//! Each `Conversation` carries its `messages: Vec<Message>` inline so
//! a single thread is one JSON document on disk. When bound to the
//! `openrouter` seam, replace `send_message`'s body to call into the
//! openrouter integration's chat helper after persisting the user message.

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::state::AppState;
use crate::store::Store;
use crate::types::current_time_unix_ms;

const STORAGE_PATH: &str = "chat_app/conversations";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    /// "user" | "assistant" | "system" — kept as a string so future roles
    /// (function, tool, …) deserialise without a code change.
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub messages: Vec<Message>,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub updated_at: u64,
}

pub trait ChatStore: Send + Sync {
    fn list(&self) -> Result<Vec<Conversation>, String>;
    fn load(&self, id: &str) -> Result<Conversation, String>;
    fn save(&self, conv: &Conversation) -> Result<(), String>;
    fn delete(&self, id: &str) -> Result<bool, String>;
}

pub struct JsonChatStore<'a> {
    store: &'a Store,
}

impl<'a> JsonChatStore<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    fn dir(&self) -> std::path::PathBuf {
        self.store.layout().root().join(STORAGE_PATH)
    }

    fn path(&self, id: &str) -> std::path::PathBuf {
        let safe = crate::store::sanitise_filename(id);
        self.dir().join(format!("{safe}.json"))
    }
}

impl<'a> ChatStore for JsonChatStore<'a> {
    fn list(&self) -> Result<Vec<Conversation>, String> {
        let dir = self.dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut convs = Vec::new();
        for entry in std::fs::read_dir(&dir).map_err(|e| format!("read dir: {e}"))? {
            let entry = entry.map_err(|e| format!("dir entry: {e}"))?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let bytes = std::fs::read(&path).map_err(|e| format!("read {path:?}: {e}"))?;
            // List view skips the messages payload to keep the response
            // small. Callers fetch the full thread via `load`.
            let mut conv: Conversation =
                serde_json::from_slice(&bytes).map_err(|e| format!("parse {path:?}: {e}"))?;
            conv.messages.clear();
            convs.push(conv);
        }
        convs.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(convs)
    }

    fn load(&self, id: &str) -> Result<Conversation, String> {
        let path = self.path(id);
        let bytes = std::fs::read(&path).map_err(|e| format!("read {path:?}: {e}"))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("parse: {e}"))
    }

    fn save(&self, conv: &Conversation) -> Result<(), String> {
        std::fs::create_dir_all(self.dir()).map_err(|e| format!("mkdir: {e}"))?;
        let json = serde_json::to_string_pretty(conv).map_err(|e| format!("encode: {e}"))?;
        let path = self.path(&conv.id);
        std::fs::write(&path, json).map_err(|e| format!("write {path:?}: {e}"))
    }

    fn delete(&self, id: &str) -> Result<bool, String> {
        let path = self.path(id);
        if !path.exists() {
            return Ok(false);
        }
        std::fs::remove_file(&path).map_err(|e| format!("delete {path:?}: {e}"))?;
        Ok(true)
    }
}

fn build_store<'a>(store: &'a Store) -> Box<dyn ChatStore + 'a> {
    Box::new(JsonChatStore::new(store))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveConversationPayload {
    /// `None` → create a new conversation; `Some(id)` → upsert that one.
    #[serde(default)]
    pub id: Option<String>,
    pub title: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendMessagePayload {
    pub conversation_id: String,
    pub content: String,
}

#[tauri::command]
pub fn list_conversations(state: State<'_, AppState>) -> Result<Vec<Conversation>, String> {
    let store = state.store();
    build_store(store).list()
}

#[tauri::command]
pub fn load_conversation(
    state: State<'_, AppState>,
    payload: crate::types::EntityIdPayload,
) -> Result<Conversation, String> {
    let store = state.store();
    build_store(store).load(&payload.id)
}

#[tauri::command]
pub fn save_conversation(
    state: State<'_, AppState>,
    payload: SaveConversationPayload,
) -> Result<Conversation, String> {
    let store = state.store();
    let now = current_time_unix_ms();
    let id = payload.id.unwrap_or_else(|| format!("conv-{now}"));
    let conv = match build_store(store).load(&id) {
        Ok(mut existing) => {
            existing.title = payload.title;
            existing.updated_at = now;
            existing
        }
        Err(_) => Conversation {
            id,
            title: payload.title,
            messages: Vec::new(),
            created_at: now,
            updated_at: now,
        },
    };
    build_store(store).save(&conv)?;
    Ok(conv)
}

#[tauri::command]
pub fn delete_conversation(
    state: State<'_, AppState>,
    payload: crate::types::EntityIdPayload,
) -> Result<bool, String> {
    let store = state.store();
    build_store(store).delete(&payload.id)
}

#[tauri::command]
pub fn send_message(
    state: State<'_, AppState>,
    payload: SendMessagePayload,
) -> Result<Conversation, String> {
    let store = state.store();
    let mut conv = build_store(store).load(&payload.conversation_id)?;
    let now = current_time_unix_ms();
    conv.messages.push(Message {
        id: format!("msg-{now}-u"),
        role: "user".into(),
        content: payload.content,
        created_at: now,
    });

    // FILL: assistant turn handling
    //
    // Default scaffold echoes the user's last message back so the UI
    // demonstrates a full round-trip without external dependencies. Replace
    // this with a real assistant turn when bound to `openrouter`:
    //
    //   let reply = crate::integrations::openrouter::chat_completion(
    //       &state, conv.messages.clone()
    //   ).await?;
    //   conv.messages.push(Message {
    //       id: format!("msg-{}", current_time_unix_ms()),
    //       role: "assistant".into(),
    //       content: reply,
    //       created_at: current_time_unix_ms(),
    //   });
    //
    // FILL: system prompt — prepend a system message to the history sent to
    // the LLM (e.g. "You are a concise research assistant.") if the user's
    // request implies one.
    let echo = conv
        .messages
        .last()
        .map(|m| format!("(echo) {}", m.content))
        .unwrap_or_default();
    conv.messages.push(Message {
        id: format!("msg-{now}-a"),
        role: "assistant".into(),
        content: echo,
        created_at: now,
    });
    conv.updated_at = now;
    build_store(store).save(&conv)?;
    Ok(conv)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn store_in(dir: &std::path::Path) -> Store {
        let s = Store::new(dir.to_path_buf());
        s.init_workspace().unwrap();
        s
    }

    #[test]
    fn json_chat_store_round_trips() {
        let tmp = tempdir().unwrap();
        let store = store_in(tmp.path());
        let cs = JsonChatStore::new(&store);
        let conv = Conversation {
            id: "c1".into(),
            title: "First".into(),
            messages: vec![Message {
                id: "m1".into(),
                role: "user".into(),
                content: "hi".into(),
                created_at: 1,
            }],
            created_at: 1,
            updated_at: 2,
        };
        cs.save(&conv).unwrap();
        let back = cs.load("c1").unwrap();
        assert_eq!(back.messages.len(), 1);
        assert_eq!(back.messages[0].content, "hi");
    }

    #[test]
    fn list_strips_messages_for_summary_view() {
        let tmp = tempdir().unwrap();
        let store = store_in(tmp.path());
        let cs = JsonChatStore::new(&store);
        cs.save(&Conversation {
            id: "c1".into(),
            title: "First".into(),
            messages: vec![Message {
                id: "m1".into(),
                role: "user".into(),
                content: "hi".into(),
                created_at: 1,
            }],
            created_at: 1,
            updated_at: 2,
        })
        .unwrap();
        let listed = cs.list().unwrap();
        assert_eq!(listed.len(), 1);
        // List view drops the message payload — clients re-fetch via load.
        assert!(listed[0].messages.is_empty());
        assert_eq!(listed[0].title, "First");
    }

    #[test]
    fn list_returns_empty_when_dir_missing() {
        let tmp = tempdir().unwrap();
        let store = store_in(tmp.path());
        let cs = JsonChatStore::new(&store);
        assert!(cs.list().unwrap().is_empty());
    }
}
