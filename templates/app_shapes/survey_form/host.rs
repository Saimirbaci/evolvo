//! survey_form — host-side scaffold.
//!
//! Drop into `app/src-tauri/src/app_shapes/survey_form.rs`. Register the
//! four `#[tauri::command]` functions in
//! `app/src-tauri/src/lib.rs::run()`'s `invoke_handler!` list.
//!
//! `Response.values` is `serde_json::Value` so the form's field schema —
//! declared in `ui_app.rs` at `// FILL: form schema` — can change without
//! a host-side type change.

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use tauri::State;

use crate::state::AppState;
use crate::store::Store;
use crate::types::current_time_unix_ms;

const STORAGE_PATH: &str = "survey_form/responses";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub id: String,
    /// Keys are field ids declared by `ui_app.rs`'s field schema. Values
    /// are the user's submission, typed by JSON's native types.
    #[serde(default)]
    pub values: Json,
    #[serde(default)]
    pub submitted_at: u64,
}

pub trait ResponseStore: Send + Sync {
    fn list(&self) -> Result<Vec<Response>, String>;
    fn load(&self, id: &str) -> Result<Response, String>;
    fn save(&self, response: &Response) -> Result<(), String>;
    fn delete(&self, id: &str) -> Result<bool, String>;
}

pub struct JsonResponseStore<'a> {
    store: &'a Store,
}

impl<'a> JsonResponseStore<'a> {
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

impl<'a> ResponseStore for JsonResponseStore<'a> {
    fn list(&self) -> Result<Vec<Response>, String> {
        let dir = self.dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&dir).map_err(|e| format!("read dir: {e}"))? {
            let entry = entry.map_err(|e| format!("dir entry: {e}"))?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let bytes = std::fs::read(&path).map_err(|e| format!("read {path:?}: {e}"))?;
            let r: Response =
                serde_json::from_slice(&bytes).map_err(|e| format!("parse {path:?}: {e}"))?;
            out.push(r);
        }
        out.sort_by(|a, b| b.submitted_at.cmp(&a.submitted_at));
        Ok(out)
    }

    fn load(&self, id: &str) -> Result<Response, String> {
        let path = self.path(id);
        let bytes = std::fs::read(&path).map_err(|e| format!("read {path:?}: {e}"))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("parse: {e}"))
    }

    fn save(&self, response: &Response) -> Result<(), String> {
        std::fs::create_dir_all(self.dir()).map_err(|e| format!("mkdir: {e}"))?;
        let json = serde_json::to_string_pretty(response).map_err(|e| format!("encode: {e}"))?;
        let path = self.path(&response.id);
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

fn build_store<'a>(store: &'a Store) -> Box<dyn ResponseStore + 'a> {
    Box::new(JsonResponseStore::new(store))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveResponsePayload {
    pub values: Json,
}

#[tauri::command]
pub fn list_responses(state: State<'_, AppState>) -> Result<Vec<Response>, String> {
    let store = state.store();
    build_store(store).list()
}

#[tauri::command]
pub fn load_response(
    state: State<'_, AppState>,
    payload: crate::types::EntityIdPayload,
) -> Result<Response, String> {
    let store = state.store();
    build_store(store).load(&payload.id)
}

#[tauri::command]
pub fn save_response(
    state: State<'_, AppState>,
    payload: SaveResponsePayload,
) -> Result<Response, String> {
    let store = state.store();
    let now = current_time_unix_ms();
    let response = Response {
        id: format!("resp-{now}"),
        values: payload.values,
        submitted_at: now,
    };
    build_store(store).save(&response)?;
    Ok(response)
}

#[tauri::command]
pub fn delete_response(
    state: State<'_, AppState>,
    payload: crate::types::EntityIdPayload,
) -> Result<bool, String> {
    let store = state.store();
    build_store(store).delete(&payload.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    fn store_in(dir: &std::path::Path) -> Store {
        let s = Store::new(dir.to_path_buf());
        s.init_workspace().unwrap();
        s
    }

    #[test]
    fn json_response_store_round_trips() {
        let tmp = tempdir().unwrap();
        let store = store_in(tmp.path());
        let rs = JsonResponseStore::new(&store);
        let r = Response {
            id: "resp-1".into(),
            values: json!({ "name": "Ada", "agree": true, "age": 36 }),
            submitted_at: 42,
        };
        rs.save(&r).unwrap();
        let back = rs.load("resp-1").unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn list_sorts_newest_first() {
        let tmp = tempdir().unwrap();
        let store = store_in(tmp.path());
        let rs = JsonResponseStore::new(&store);
        rs.save(&Response {
            id: "a".into(),
            values: json!({}),
            submitted_at: 1,
        })
        .unwrap();
        rs.save(&Response {
            id: "b".into(),
            values: json!({}),
            submitted_at: 2,
        })
        .unwrap();
        let listed = rs.list().unwrap();
        assert_eq!(listed[0].id, "b");
    }

    #[test]
    fn delete_returns_false_when_missing() {
        let tmp = tempdir().unwrap();
        let store = store_in(tmp.path());
        let rs = JsonResponseStore::new(&store);
        assert!(!rs.delete("nope").unwrap());
    }
}
