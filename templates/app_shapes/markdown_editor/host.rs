//! markdown_editor — host-side scaffold.
//!
//! Drop into `app/src-tauri/src/app_shapes/markdown_editor.rs`, register
//! the four `#[tauri::command]` functions in
//! `app/src-tauri/src/lib.rs::run()`'s `invoke_handler!` list, then walk the
//! `// FILL:` markers below to specialise the scaffold for the current
//! iteration's user request.
//!
//! The default `DocumentStore` implementation persists JSON-per-entity via
//! the existing host `Store` (no extra dependency). When
//! `plan.templates.backendBindings.markdown_editor` is set, replace
//! `JsonDocumentStore` with the integration's adapter (see the scaffold's
//! SKILL.md → "Backend seams").

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::state::AppState;
use crate::store::Store;
use crate::types::current_time_unix_ms;

const STORAGE_PATH: &str = "markdown_editor/documents";

/// One document the editor renders. Wire-type, camelCase on serialisation.
/// FILL: domain-specific document fields — add `tags: Vec<String>`,
/// `archived: bool`, `slug: String`, etc., as the user's request implies.
/// Default-able fields use `#[serde(default)]` so older JSON keeps loading.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Document {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub updated_at: u64,
}

/// Object-safe persistence trait. The four Tauri commands always go through
/// this — replacing it is how a backend seam swaps storage without touching
/// the UI.
pub trait DocumentStore: Send + Sync {
    fn list(&self) -> Result<Vec<Document>, String>;
    fn load(&self, id: &str) -> Result<Document, String>;
    fn save(&self, doc: &Document) -> Result<(), String>;
    fn delete(&self, id: &str) -> Result<bool, String>;
}

/// Default impl — JSON file per document under
/// `<workspace>/markdown_editor/documents/{id}.json`. Composes with the
/// existing host `Store` so the bytes-on-disk layout matches the rest of
/// the app.
pub struct JsonDocumentStore<'a> {
    store: &'a Store,
}

impl<'a> JsonDocumentStore<'a> {
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

impl<'a> DocumentStore for JsonDocumentStore<'a> {
    fn list(&self) -> Result<Vec<Document>, String> {
        let dir = self.dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut docs = Vec::new();
        for entry in std::fs::read_dir(&dir).map_err(|e| format!("read dir: {e}"))? {
            let entry = entry.map_err(|e| format!("dir entry: {e}"))?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let bytes = std::fs::read(&path).map_err(|e| format!("read {path:?}: {e}"))?;
            let doc: Document = serde_json::from_slice(&bytes)
                .map_err(|e| format!("parse {path:?}: {e}"))?;
            docs.push(doc);
        }
        // Newest first, matches the user's mental model of "most recent on top".
        docs.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(docs)
    }

    fn load(&self, id: &str) -> Result<Document, String> {
        let path = self.path(id);
        let bytes = std::fs::read(&path).map_err(|e| format!("read {path:?}: {e}"))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("parse: {e}"))
    }

    fn save(&self, doc: &Document) -> Result<(), String> {
        std::fs::create_dir_all(self.dir()).map_err(|e| format!("mkdir: {e}"))?;
        let json = serde_json::to_string_pretty(doc).map_err(|e| format!("encode: {e}"))?;
        let path = self.path(&doc.id);
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

/// Build the active store for this iteration. Replace the body when a
/// backend seam is bound (see SKILL.md → Backend seams).
fn build_store<'a>(store: &'a Store) -> Box<dyn DocumentStore + 'a> {
    Box::new(JsonDocumentStore::new(store))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveDocumentPayload {
    /// `None` → create; `Some(id)` → update in place.
    #[serde(default)]
    pub id: Option<String>,
    pub title: String,
    #[serde(default)]
    pub body: String,
}

#[tauri::command]
pub fn list_documents(state: State<'_, AppState>) -> Result<Vec<Document>, String> {
    let store = state.store();
    build_store(store).list()
}

#[tauri::command]
pub fn load_document(
    state: State<'_, AppState>,
    payload: crate::types::EntityIdPayload,
) -> Result<Document, String> {
    let store = state.store();
    build_store(store).load(&payload.id)
}

#[tauri::command]
pub fn save_document(
    state: State<'_, AppState>,
    payload: SaveDocumentPayload,
) -> Result<Document, String> {
    let store = state.store();
    let now = current_time_unix_ms();
    let id = payload.id.unwrap_or_else(|| format!("doc-{now}"));
    let doc = Document {
        id,
        title: payload.title,
        body: payload.body,
        updated_at: now,
        // FILL: domain-specific document fields — populate from payload.
    };
    build_store(store).save(&doc)?;
    Ok(doc)
}

#[tauri::command]
pub fn delete_document(
    state: State<'_, AppState>,
    payload: crate::types::EntityIdPayload,
) -> Result<bool, String> {
    let store = state.store();
    build_store(store).delete(&payload.id)
}

// FILL: domain-specific document actions
//
// Add additional `#[tauri::command]` functions here for any non-CRUD
// behaviour the user's request implies. Examples:
//
//   #[tauri::command]
//   pub fn publish_document(
//       state: State<'_, AppState>,
//       payload: crate::types::EntityIdPayload,
//   ) -> Result<String, String> { /* POST to a blog endpoint, return URL */ }
//
// Every new command MUST also be added to `invoke_handler!` in lib.rs.

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
    fn json_document_store_round_trips() {
        let tmp = tempdir().unwrap();
        let store = store_in(tmp.path());
        let ds = JsonDocumentStore::new(&store);
        let doc = Document {
            id: "doc-1".into(),
            title: "Hello".into(),
            body: "world".into(),
            updated_at: 42,
        };
        ds.save(&doc).unwrap();
        let back = ds.load("doc-1").unwrap();
        assert_eq!(back, doc);
        let listed = ds.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].title, "Hello");
        assert!(ds.delete("doc-1").unwrap());
        assert!(!ds.delete("doc-1").unwrap()); // second delete: not found
    }

    #[test]
    fn list_returns_empty_when_dir_missing() {
        let tmp = tempdir().unwrap();
        let store = store_in(tmp.path());
        let ds = JsonDocumentStore::new(&store);
        assert!(ds.list().unwrap().is_empty());
    }

    #[test]
    fn list_sorts_newest_first() {
        let tmp = tempdir().unwrap();
        let store = store_in(tmp.path());
        let ds = JsonDocumentStore::new(&store);
        ds.save(&Document {
            id: "a".into(),
            title: "old".into(),
            body: "".into(),
            updated_at: 1,
        })
        .unwrap();
        ds.save(&Document {
            id: "b".into(),
            title: "new".into(),
            body: "".into(),
            updated_at: 2,
        })
        .unwrap();
        let listed = ds.list().unwrap();
        assert_eq!(listed[0].id, "b");
        assert_eq!(listed[1].id, "a");
    }
}
