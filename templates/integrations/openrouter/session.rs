//! Chat session model + JSON persistence.
//!
//! One file per session at `<workspace>/integrations/openrouter/sessions/<id>.json`.
//! Sessions are mutable: every `append_message` rewrites the whole file
//! atomically (write to `<id>.json.tmp`, then rename). Good enough for
//! single-user desktop usage; upgrade to SQLite if you exceed ~1k sessions or
//! need concurrent writers.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const DEFAULT_MODEL: &str = "openai/gpt-4o-mini";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub role: Role,
    pub content: String,
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSession {
    pub id: String,
    pub app_name: String,
    pub model: String,
    pub title: Option<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    #[serde(default)]
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: String,
    pub app_name: String,
    pub model: String,
    pub title: Option<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub message_count: usize,
}

impl From<&ChatSession> for SessionSummary {
    fn from(s: &ChatSession) -> Self {
        Self {
            id: s.id.clone(),
            app_name: s.app_name.clone(),
            model: s.model.clone(),
            title: s.title.clone(),
            created_at_ms: s.created_at_ms,
            updated_at_ms: s.updated_at_ms,
            message_count: s.messages.len(),
        }
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Builds the system prompt injected on every request. Kept pure so the app
/// name can change between requests without rewriting the stored history.
pub fn build_system_prompt(app_name: &str) -> String {
    format!(
        "You are the in-app AI assistant for \"{app_name}\", a desktop application the user is building on the Evolvo platform. \
Help the user with questions about the app, generating content inside it, and reasoning about their data. \
Be concise. If the user asks you to take an action inside the app, explain what they should click; you do not have tool access to act on their behalf."
    )
}

pub struct SessionStore {
    root: PathBuf,
}

impl SessionStore {
    pub fn new(workspace_root: &Path) -> Self {
        Self {
            root: workspace_root.join("integrations/openrouter/sessions"),
        }
    }

    fn ensure(&self) -> std::io::Result<()> {
        fs::create_dir_all(&self.root)
    }

    fn path_for(&self, id: &str) -> PathBuf {
        self.root.join(format!("{id}.json"))
    }

    pub fn create(&self, app_name: String, model: Option<String>) -> std::io::Result<ChatSession> {
        self.ensure()?;
        let now = now_ms();
        let session = ChatSession {
            id: Uuid::new_v4().to_string(),
            app_name,
            model: model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            title: None,
            created_at_ms: now,
            updated_at_ms: now,
            messages: Vec::new(),
        };
        self.save(&session)?;
        Ok(session)
    }

    pub fn load(&self, id: &str) -> std::io::Result<ChatSession> {
        let data = fs::read_to_string(self.path_for(id))?;
        serde_json::from_str(&data)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    pub fn save(&self, session: &ChatSession) -> std::io::Result<()> {
        self.ensure()?;
        let path = self.path_for(&session.id);
        let tmp = path.with_extension("json.tmp");
        let body = serde_json::to_string_pretty(session)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        fs::write(&tmp, body)?;
        fs::rename(tmp, path)?;
        Ok(())
    }

    pub fn delete(&self, id: &str) -> std::io::Result<()> {
        let path = self.path_for(id);
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }

    pub fn list(&self) -> std::io::Result<Vec<SessionSummary>> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            if let Ok(session) = fs::read_to_string(&path)
                .and_then(|s| serde_json::from_str::<ChatSession>(&s)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e)))
            {
                out.push(SessionSummary::from(&session));
            }
        }
        out.sort_by(|a, b| b.updated_at_ms.cmp(&a.updated_at_ms));
        Ok(out)
    }

    pub fn append(&self, id: &str, message: Message) -> std::io::Result<ChatSession> {
        let mut session = self.load(id)?;
        session.messages.push(message);
        session.updated_at_ms = now_ms();
        if session.title.is_none() {
            if let Some(first_user) = session
                .messages
                .iter()
                .find(|m| m.role == Role::User)
                .map(|m| m.content.clone())
            {
                let title: String = first_user.chars().take(60).collect();
                session.title = Some(title);
            }
        }
        self.save(&session)?;
        Ok(session)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn round_trip_session() {
        let dir = tempdir().unwrap();
        let store = SessionStore::new(dir.path());
        let session = store.create("TestApp".into(), None).unwrap();
        assert_eq!(session.app_name, "TestApp");
        assert_eq!(session.model, DEFAULT_MODEL);

        store
            .append(
                &session.id,
                Message {
                    role: Role::User,
                    content: "hello".into(),
                    created_at_ms: now_ms(),
                },
            )
            .unwrap();

        let loaded = store.load(&session.id).unwrap();
        assert_eq!(loaded.messages.len(), 1);
        assert_eq!(loaded.title.as_deref(), Some("hello"));

        let summaries = store.list().unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].message_count, 1);

        store.delete(&session.id).unwrap();
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn system_prompt_includes_app_name() {
        let p = build_system_prompt("Bookkeeper");
        assert!(p.contains("Bookkeeper"));
        assert!(p.contains("Evolvo"));
    }
}
