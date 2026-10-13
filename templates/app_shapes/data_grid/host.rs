//! data_grid — host-side scaffold.
//!
//! Drop into `app/src-tauri/src/app_shapes/data_grid.rs`. Register the
//! four `#[tauri::command]` functions in
//! `app/src-tauri/src/lib.rs::run()`'s `invoke_handler!` list.
//!
//! Rows are schemaless — `cells` is a `serde_json::Value::Object` so the
//! agent can declare any columns the user wants without changing the
//! wire type. The `// FILL: column schema` marker in `ui_app.rs` is
//! where the iteration-specific column list goes.

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use tauri::State;

use crate::state::AppState;
use crate::store::Store;
use crate::types::current_time_unix_ms;

const STORAGE_PATH: &str = "data_grid/rows";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub id: String,
    /// Schemaless — keys are column ids, values are the cell content.
    /// Stays JSON so swapping the UI's column schema doesn't require a
    /// host-side type change.
    #[serde(default)]
    pub cells: Json,
    #[serde(default)]
    pub updated_at: u64,
}

pub trait RowStore: Send + Sync {
    fn list(&self) -> Result<Vec<Row>, String>;
    fn load(&self, id: &str) -> Result<Row, String>;
    fn save(&self, row: &Row) -> Result<(), String>;
    fn delete(&self, id: &str) -> Result<bool, String>;
}

pub struct JsonRowStore<'a> {
    store: &'a Store,
}

impl<'a> JsonRowStore<'a> {
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

impl<'a> RowStore for JsonRowStore<'a> {
    fn list(&self) -> Result<Vec<Row>, String> {
        let dir = self.dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut rows = Vec::new();
        for entry in std::fs::read_dir(&dir).map_err(|e| format!("read dir: {e}"))? {
            let entry = entry.map_err(|e| format!("dir entry: {e}"))?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let bytes = std::fs::read(&path).map_err(|e| format!("read {path:?}: {e}"))?;
            let row: Row =
                serde_json::from_slice(&bytes).map_err(|e| format!("parse {path:?}: {e}"))?;
            rows.push(row);
        }
        rows.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(rows)
    }

    fn load(&self, id: &str) -> Result<Row, String> {
        let path = self.path(id);
        let bytes = std::fs::read(&path).map_err(|e| format!("read {path:?}: {e}"))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("parse: {e}"))
    }

    fn save(&self, row: &Row) -> Result<(), String> {
        std::fs::create_dir_all(self.dir()).map_err(|e| format!("mkdir: {e}"))?;
        let json = serde_json::to_string_pretty(row).map_err(|e| format!("encode: {e}"))?;
        let path = self.path(&row.id);
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

fn build_store<'a>(store: &'a Store) -> Box<dyn RowStore + 'a> {
    Box::new(JsonRowStore::new(store))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveRowPayload {
    /// `None` → create; `Some(id)` → update in place.
    #[serde(default)]
    pub id: Option<String>,
    pub cells: Json,
}

#[tauri::command]
pub fn list_rows(state: State<'_, AppState>) -> Result<Vec<Row>, String> {
    let store = state.store();
    build_store(store).list()
}

#[tauri::command]
pub fn load_row(
    state: State<'_, AppState>,
    payload: crate::types::EntityIdPayload,
) -> Result<Row, String> {
    let store = state.store();
    build_store(store).load(&payload.id)
}

#[tauri::command]
pub fn save_row(state: State<'_, AppState>, payload: SaveRowPayload) -> Result<Row, String> {
    let store = state.store();
    let now = current_time_unix_ms();
    let id = payload.id.unwrap_or_else(|| format!("row-{now}"));
    let row = Row {
        id,
        cells: payload.cells,
        updated_at: now,
    };
    build_store(store).save(&row)?;
    Ok(row)
}

#[tauri::command]
pub fn delete_row(
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
    fn json_row_store_round_trips() {
        let tmp = tempdir().unwrap();
        let store = store_in(tmp.path());
        let rs = JsonRowStore::new(&store);
        let row = Row {
            id: "row-1".into(),
            cells: json!({ "name": "Ada", "email": "ada@example.com" }),
            updated_at: 42,
        };
        rs.save(&row).unwrap();
        let back = rs.load("row-1").unwrap();
        assert_eq!(back, row);
        let listed = rs.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert!(rs.delete("row-1").unwrap());
        assert!(rs.list().unwrap().is_empty());
    }

    #[test]
    fn list_returns_empty_when_dir_missing() {
        let tmp = tempdir().unwrap();
        let store = store_in(tmp.path());
        let rs = JsonRowStore::new(&store);
        assert!(rs.list().unwrap().is_empty());
    }
}
