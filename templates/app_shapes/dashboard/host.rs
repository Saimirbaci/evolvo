//! dashboard — host-side scaffold.
//!
//! Drop into `app/src-tauri/src/app_shapes/dashboard.rs`. Register the
//! five `#[tauri::command]` functions in
//! `app/src-tauri/src/lib.rs::run()`'s `invoke_handler!` list.
//!
//! Each `Widget` carries its latest fetched reading inline so the
//! dashboard renders in one storage round-trip. Time-series storage
//! belongs to the sqlite backend seam (see SKILL.md).

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use tauri::State;

use crate::state::AppState;
use crate::store::Store;
use crate::types::current_time_unix_ms;

const STORAGE_PATH: &str = "dashboard/widgets";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Widget {
    pub id: String,
    pub title: String,
    /// "number" | "text" | "bar_chart" | "line_chart" | … — kept as a
    /// string so additional kinds slot in via the FILL marker without a
    /// type change.
    pub kind: String,
    #[serde(default)]
    pub source_url: Option<String>,
    #[serde(default)]
    pub refresh_interval_ms: Option<u64>,
    #[serde(default)]
    pub ordinal: u32,
    #[serde(default)]
    pub config: Json,
    #[serde(default)]
    pub latest_value: Option<Json>,
    #[serde(default)]
    pub latest_fetched_at: Option<u64>,
}

pub trait DashboardStore: Send + Sync {
    fn list(&self) -> Result<Vec<Widget>, String>;
    fn load(&self, id: &str) -> Result<Widget, String>;
    fn save(&self, widget: &Widget) -> Result<(), String>;
    fn delete(&self, id: &str) -> Result<bool, String>;
}

pub struct JsonDashboardStore<'a> {
    store: &'a Store,
}

impl<'a> JsonDashboardStore<'a> {
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

impl<'a> DashboardStore for JsonDashboardStore<'a> {
    fn list(&self) -> Result<Vec<Widget>, String> {
        let dir = self.dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut widgets = Vec::new();
        for entry in std::fs::read_dir(&dir).map_err(|e| format!("read dir: {e}"))? {
            let entry = entry.map_err(|e| format!("dir entry: {e}"))?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let bytes = std::fs::read(&path).map_err(|e| format!("read {path:?}: {e}"))?;
            let w: Widget =
                serde_json::from_slice(&bytes).map_err(|e| format!("parse {path:?}: {e}"))?;
            widgets.push(w);
        }
        widgets.sort_by_key(|w| w.ordinal);
        Ok(widgets)
    }

    fn load(&self, id: &str) -> Result<Widget, String> {
        let path = self.path(id);
        let bytes = std::fs::read(&path).map_err(|e| format!("read {path:?}: {e}"))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("parse: {e}"))
    }

    fn save(&self, widget: &Widget) -> Result<(), String> {
        std::fs::create_dir_all(self.dir()).map_err(|e| format!("mkdir: {e}"))?;
        let json = serde_json::to_string_pretty(widget).map_err(|e| format!("encode: {e}"))?;
        let path = self.path(&widget.id);
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

fn build_store<'a>(store: &'a Store) -> Box<dyn DashboardStore + 'a> {
    Box::new(JsonDashboardStore::new(store))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveWidgetPayload {
    #[serde(default)]
    pub id: Option<String>,
    pub title: String,
    pub kind: String,
    #[serde(default)]
    pub source_url: Option<String>,
    #[serde(default)]
    pub refresh_interval_ms: Option<u64>,
    #[serde(default)]
    pub config: Json,
}

#[tauri::command]
pub fn list_widgets(state: State<'_, AppState>) -> Result<Vec<Widget>, String> {
    let store = state.store();
    build_store(store).list()
}

#[tauri::command]
pub fn load_widget(
    state: State<'_, AppState>,
    payload: crate::types::EntityIdPayload,
) -> Result<Widget, String> {
    let store = state.store();
    build_store(store).load(&payload.id)
}

#[tauri::command]
pub fn save_widget(
    state: State<'_, AppState>,
    payload: SaveWidgetPayload,
) -> Result<Widget, String> {
    let store = state.store();
    let id = payload
        .id
        .unwrap_or_else(|| format!("w-{}", current_time_unix_ms()));
    let mut widget = match build_store(store).load(&id) {
        Ok(existing) => existing,
        Err(_) => Widget {
            id: id.clone(),
            title: String::new(),
            kind: String::new(),
            source_url: None,
            refresh_interval_ms: None,
            ordinal: 0,
            config: Json::Null,
            latest_value: None,
            latest_fetched_at: None,
        },
    };
    widget.title = payload.title;
    widget.kind = payload.kind;
    widget.source_url = payload.source_url;
    widget.refresh_interval_ms = payload.refresh_interval_ms;
    widget.config = payload.config;
    build_store(store).save(&widget)?;
    Ok(widget)
}

#[tauri::command]
pub fn delete_widget(
    state: State<'_, AppState>,
    payload: crate::types::EntityIdPayload,
) -> Result<bool, String> {
    let store = state.store();
    build_store(store).delete(&payload.id)
}

#[tauri::command]
pub fn refresh_widget(
    state: State<'_, AppState>,
    payload: crate::types::EntityIdPayload,
) -> Result<Widget, String> {
    let store = state.store();
    let mut widget = build_store(store).load(&payload.id)?;
    let now = current_time_unix_ms();

    // FILL: refresh implementation
    //
    // Default is a no-op stamp: just records that a refresh was attempted.
    // When bound to `http_json`, replace with:
    //
    //   if let Some(url) = &widget.source_url {
    //       let raw = crate::integrations::http_json::get_json(&state, url).await?;
    //       widget.latest_value = Some(raw);
    //   }
    //
    // For widgets whose JSON response is nested, extract the relevant
    // path from `widget.config` (e.g. a `path: "$.data[0].value"` jsonpath).
    widget.latest_fetched_at = Some(now);

    build_store(store).save(&widget)?;
    Ok(widget)
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
    fn json_dashboard_store_round_trips() {
        let tmp = tempdir().unwrap();
        let store = store_in(tmp.path());
        let ds = JsonDashboardStore::new(&store);
        let w = Widget {
            id: "w1".into(),
            title: "Sales".into(),
            kind: "number".into(),
            source_url: Some("https://example.com/sales".into()),
            refresh_interval_ms: Some(60_000),
            ordinal: 0,
            config: json!({}),
            latest_value: Some(json!(42)),
            latest_fetched_at: Some(1),
        };
        ds.save(&w).unwrap();
        let back = ds.load("w1").unwrap();
        assert_eq!(back.latest_value, Some(json!(42)));
    }

    #[test]
    fn list_sorts_by_ordinal() {
        let tmp = tempdir().unwrap();
        let store = store_in(tmp.path());
        let ds = JsonDashboardStore::new(&store);
        for (id, ord) in [("z", 2u32), ("a", 0), ("m", 1)] {
            ds.save(&Widget {
                id: id.into(),
                title: id.into(),
                kind: "number".into(),
                source_url: None,
                refresh_interval_ms: None,
                ordinal: ord,
                config: Json::Null,
                latest_value: None,
                latest_fetched_at: None,
            })
            .unwrap();
        }
        let listed: Vec<_> = ds.list().unwrap().into_iter().map(|w| w.id).collect();
        assert_eq!(listed, vec!["a", "m", "z"]);
    }
}
