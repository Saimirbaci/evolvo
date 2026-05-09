//! calendar — host-side scaffold.
//!
//! Drop into `app/src-tauri/src/app_shapes/calendar.rs`. Register the
//! four `#[tauri::command]` functions in
//! `app/src-tauri/src/lib.rs::run()`'s `invoke_handler!` list.
//!
//! Default JSON store keeps one event per file. `list_in_range` scans
//! the directory; sqlite seams should override it with a SQL range
//! query.

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::state::AppState;
use crate::store::Store;
use crate::types::current_time_unix_ms;

const STORAGE_PATH: &str = "calendar/events";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CalendarEvent {
    pub id: String,
    pub title: String,
    pub start_ms: u64,
    pub end_ms: u64,
    #[serde(default)]
    pub all_day: bool,
    #[serde(default)]
    pub notes: Option<String>,
}

pub trait CalendarStore: Send + Sync {
    fn list_in_range(&self, start_ms: u64, end_ms: u64) -> Result<Vec<CalendarEvent>, String>;
    fn load(&self, id: &str) -> Result<CalendarEvent, String>;
    fn save(&self, event: &CalendarEvent) -> Result<(), String>;
    fn delete(&self, id: &str) -> Result<bool, String>;
}

pub struct JsonCalendarStore<'a> {
    store: &'a Store,
}

impl<'a> JsonCalendarStore<'a> {
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

    fn list_all(&self) -> Result<Vec<CalendarEvent>, String> {
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
            let e: CalendarEvent =
                serde_json::from_slice(&bytes).map_err(|e| format!("parse {path:?}: {e}"))?;
            events.push(e);
        }
        Ok(events)
    }
}

impl<'a> CalendarStore for JsonCalendarStore<'a> {
    fn list_in_range(&self, start_ms: u64, end_ms: u64) -> Result<Vec<CalendarEvent>, String> {
        let mut events = self.list_all()?;
        // Overlap test: event_start < range_end AND event_end > range_start.
        events.retain(|e| e.start_ms < end_ms && e.end_ms > start_ms);
        events.sort_by_key(|e| e.start_ms);
        Ok(events)
    }

    fn load(&self, id: &str) -> Result<CalendarEvent, String> {
        let path = self.path(id);
        let bytes = std::fs::read(&path).map_err(|e| format!("read {path:?}: {e}"))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("parse: {e}"))
    }

    fn save(&self, event: &CalendarEvent) -> Result<(), String> {
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

fn build_store<'a>(store: &'a Store) -> Box<dyn CalendarStore + 'a> {
    Box::new(JsonCalendarStore::new(store))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListEventsInRangeArgs {
    pub start_ms: u64,
    pub end_ms: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveEventPayload {
    #[serde(default)]
    pub id: Option<String>,
    pub title: String,
    pub start_ms: u64,
    pub end_ms: u64,
    #[serde(default)]
    pub all_day: bool,
    #[serde(default)]
    pub notes: Option<String>,
}

#[tauri::command]
pub fn list_events_in_range(
    state: State<'_, AppState>,
    args: ListEventsInRangeArgs,
) -> Result<Vec<CalendarEvent>, String> {
    let store = state.store();
    build_store(store).list_in_range(args.start_ms, args.end_ms)
}

#[tauri::command]
pub fn load_event(
    state: State<'_, AppState>,
    payload: crate::types::EntityIdPayload,
) -> Result<CalendarEvent, String> {
    let store = state.store();
    build_store(store).load(&payload.id)
}

#[tauri::command]
pub fn save_event(
    state: State<'_, AppState>,
    payload: SaveEventPayload,
) -> Result<CalendarEvent, String> {
    let store = state.store();
    let id = payload
        .id
        .unwrap_or_else(|| format!("ev-{}", current_time_unix_ms()));
    let event = CalendarEvent {
        id,
        title: payload.title,
        start_ms: payload.start_ms,
        end_ms: payload.end_ms,
        all_day: payload.all_day,
        notes: payload.notes,
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
    use tempfile::tempdir;

    fn store_in(dir: &std::path::Path) -> Store {
        let s = Store::new(dir.to_path_buf());
        s.init_workspace().unwrap();
        s
    }

    #[test]
    fn json_calendar_store_round_trips() {
        let tmp = tempdir().unwrap();
        let store = store_in(tmp.path());
        let cs = JsonCalendarStore::new(&store);
        let event = CalendarEvent {
            id: "ev-1".into(),
            title: "Standup".into(),
            start_ms: 100,
            end_ms: 200,
            all_day: false,
            notes: Some("daily".into()),
        };
        cs.save(&event).unwrap();
        let back = cs.load("ev-1").unwrap();
        assert_eq!(back, event);
    }

    #[test]
    fn list_in_range_overlaps_correctly() {
        let tmp = tempdir().unwrap();
        let store = store_in(tmp.path());
        let cs = JsonCalendarStore::new(&store);
        // Outside, before.
        cs.save(&CalendarEvent {
            id: "before".into(),
            title: "before".into(),
            start_ms: 10,
            end_ms: 20,
            all_day: false,
            notes: None,
        })
        .unwrap();
        // Overlaps the range start.
        cs.save(&CalendarEvent {
            id: "left_overlap".into(),
            title: "left".into(),
            start_ms: 80,
            end_ms: 110,
            all_day: false,
            notes: None,
        })
        .unwrap();
        // Fully inside.
        cs.save(&CalendarEvent {
            id: "inside".into(),
            title: "inside".into(),
            start_ms: 120,
            end_ms: 180,
            all_day: false,
            notes: None,
        })
        .unwrap();
        // Outside, after.
        cs.save(&CalendarEvent {
            id: "after".into(),
            title: "after".into(),
            start_ms: 300,
            end_ms: 400,
            all_day: false,
            notes: None,
        })
        .unwrap();
        let in_range = cs.list_in_range(100, 200).unwrap();
        let ids: Vec<_> = in_range.iter().map(|e| e.id.clone()).collect();
        assert_eq!(ids, vec!["left_overlap", "inside"]);
    }

    #[test]
    fn delete_returns_false_when_missing() {
        let tmp = tempdir().unwrap();
        let store = store_in(tmp.path());
        let cs = JsonCalendarStore::new(&store);
        assert!(!cs.delete("nope").unwrap());
    }
}
