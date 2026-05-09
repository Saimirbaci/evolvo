//! timeline_feed — host-side scaffold.
//!
//! Drop into `app/src-tauri/src/app_shapes/timeline_feed.rs`. Register
//! the four `#[tauri::command]` functions in
//! `app/src-tauri/src/lib.rs::run()`'s `invoke_handler!` list.
//!
//! Each event keeps two timestamps so a feed of "things that happened
//! upstream and arrived here later" can sort either by domain time
//! (`occurredAt`) or by ingestion time (`ingestedAt`).

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use tauri::State;

use crate::state::AppState;
use crate::store::Store;
use crate::types::current_time_unix_ms;

const STORAGE_PATH: &str = "timeline_feed/events";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub id: String,
    pub kind: String,
    pub summary: String,
    #[serde(default)]
    pub payload: Json,
    /// When the event happened in its source domain. Drives chronological
    /// rendering.
    pub occurred_at: u64,
    /// When the local app saw the event (always `now()` at record time).
    /// Useful for "what arrived since I last looked?" queries.
    pub ingested_at: u64,
}

pub trait EventStore: Send + Sync {
    fn list(&self) -> Result<Vec<Event>, String>;
    fn load(&self, id: &str) -> Result<Event, String>;
    fn save(&self, event: &Event) -> Result<(), String>;
    fn delete(&self, id: &str) -> Result<bool, String>;
}

pub struct JsonEventStore<'a> {
    store: &'a Store,
}

impl<'a> JsonEventStore<'a> {
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

impl<'a> EventStore for JsonEventStore<'a> {
    fn list(&self) -> Result<Vec<Event>, String> {
        let dir = self.dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut events = Vec::new();
        for entry in std::fs::read_dir(&dir).map_err(|e| format!("read dir: {e}"))? {
            let entry = entry.map_err(|e| format!("dir entry: {e}"))?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let bytes = std::fs::read(&path).map_err(|e| format!("read {path:?}: {e}"))?;
            let e: Event =
                serde_json::from_slice(&bytes).map_err(|e| format!("parse {path:?}: {e}"))?;
            events.push(e);
        }
        events.sort_by(|a, b| b.occurred_at.cmp(&a.occurred_at));
        Ok(events)
    }

    fn load(&self, id: &str) -> Result<Event, String> {
        let path = self.path(id);
        let bytes = std::fs::read(&path).map_err(|e| format!("read {path:?}: {e}"))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("parse: {e}"))
    }

    fn save(&self, event: &Event) -> Result<(), String> {
        std::fs::create_dir_all(self.dir()).map_err(|e| format!("mkdir: {e}"))?;
        let json = serde_json::to_string_pretty(event).map_err(|e| format!("encode: {e}"))?;
        let path = self.path(&event.id);
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

fn build_store<'a>(store: &'a Store) -> Box<dyn EventStore + 'a> {
    Box::new(JsonEventStore::new(store))
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ListEventsFilter {
    #[serde(default)]
    pub kinds: Vec<String>,
    /// Inclusive lower bound on `occurredAt`.
    #[serde(default)]
    pub since: Option<u64>,
    /// Exclusive upper bound on `occurredAt`.
    #[serde(default)]
    pub until: Option<u64>,
    /// Free-text search applied to `kind` + `summary`.
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordEventPayload {
    pub kind: String,
    pub summary: String,
    #[serde(default)]
    pub payload: Json,
    /// Optional — defaults to `now()` when omitted.
    #[serde(default)]
    pub occurred_at: Option<u64>,
}

#[tauri::command]
pub fn list_events(
    state: State<'_, AppState>,
    #[serde(default)] filter: Option<ListEventsFilter>,
) -> Result<Vec<Event>, String> {
    let store = state.store();
    let mut events = build_store(store).list()?;
    let f = filter.unwrap_or_default();
    if !f.kinds.is_empty() {
        events.retain(|e| f.kinds.iter().any(|k| k == &e.kind));
    }
    if let Some(s) = f.since {
        events.retain(|e| e.occurred_at >= s);
    }
    if let Some(u) = f.until {
        events.retain(|e| e.occurred_at < u);
    }
    if let Some(q) = f.q.as_deref() {
        let needle = q.to_lowercase();
        if !needle.is_empty() {
            events.retain(|e| {
                e.kind.to_lowercase().contains(&needle)
                    || e.summary.to_lowercase().contains(&needle)
            });
        }
    }
    if let Some(limit) = f.limit {
        events.truncate(limit);
    }
    Ok(events)
}

#[tauri::command]
pub fn load_event(
    state: State<'_, AppState>,
    payload: crate::types::EntityIdPayload,
) -> Result<Event, String> {
    let store = state.store();
    build_store(store).load(&payload.id)
}

#[tauri::command]
pub fn record_event(
    state: State<'_, AppState>,
    payload: RecordEventPayload,
) -> Result<Event, String> {
    let store = state.store();
    let now = current_time_unix_ms();
    let event = Event {
        id: format!("ev-{now}"),
        kind: payload.kind,
        summary: payload.summary,
        payload: payload.payload,
        occurred_at: payload.occurred_at.unwrap_or(now),
        ingested_at: now,
    };
    build_store(store).save(&event)?;
    Ok(event)
}

#[tauri::command]
pub fn delete_event(
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
    fn json_event_store_round_trips() {
        let tmp = tempdir().unwrap();
        let store = store_in(tmp.path());
        let es = JsonEventStore::new(&store);
        let e = Event {
            id: "ev-1".into(),
            kind: "deploy".into(),
            summary: "shipped v1.2.3".into(),
            payload: json!({ "version": "1.2.3" }),
            occurred_at: 100,
            ingested_at: 110,
        };
        es.save(&e).unwrap();
        let back = es.load("ev-1").unwrap();
        assert_eq!(back.summary, "shipped v1.2.3");
    }

    #[test]
    fn list_sorts_by_occurred_at_desc() {
        let tmp = tempdir().unwrap();
        let store = store_in(tmp.path());
        let es = JsonEventStore::new(&store);
        for (id, t) in [("a", 1u64), ("b", 3), ("c", 2)] {
            es.save(&Event {
                id: id.into(),
                kind: "k".into(),
                summary: id.into(),
                payload: Json::Null,
                occurred_at: t,
                ingested_at: t,
            })
            .unwrap();
        }
        let listed: Vec<_> = es.list().unwrap().into_iter().map(|e| e.id).collect();
        assert_eq!(listed, vec!["b", "c", "a"]);
    }
}
