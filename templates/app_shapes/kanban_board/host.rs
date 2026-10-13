//! kanban_board — host-side scaffold.
//!
//! Drop into `app/src-tauri/src/app_shapes/kanban_board.rs`. Register all
//! four `#[tauri::command]` functions in
//! `app/src-tauri/src/lib.rs::run()`'s `invoke_handler!` list.
//!
//! The whole board is one document on disk
//! (`<workspace>/kanban_board/board.json`). Cards live inside it. Default
//! columns are Todo / Doing / Done; replace via `// FILL: column
//! customisation` if the user's request implies a different workflow.

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::state::AppState;
use crate::store::Store;
use crate::types::current_time_unix_ms;

const STORAGE_FILE: &str = "kanban_board/board.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Card {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub body: String,
    pub column_id: String,
    pub ordinal: u32,
    #[serde(default)]
    pub updated_at: u64,
    // FILL: card-level domain fields — assignee, due_date, priority, labels.
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Column {
    pub id: String,
    pub name: String,
    pub ordinal: u32,
    pub cards: Vec<Card>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BoardState {
    pub version: u32,
    pub columns: Vec<Column>,
    #[serde(default)]
    pub updated_at: u64,
}

impl BoardState {
    pub fn default_seed() -> Self {
        // FILL: column customisation — replace with the columns the user's
        // workflow needs (Backlog / In Review / Done, etc.).
        Self {
            version: 1,
            columns: vec![
                Column {
                    id: "todo".into(),
                    name: "Todo".into(),
                    ordinal: 0,
                    cards: Vec::new(),
                },
                Column {
                    id: "doing".into(),
                    name: "Doing".into(),
                    ordinal: 1,
                    cards: Vec::new(),
                },
                Column {
                    id: "done".into(),
                    name: "Done".into(),
                    ordinal: 2,
                    cards: Vec::new(),
                },
            ],
            updated_at: 0,
        }
    }
}

pub trait BoardStore: Send + Sync {
    fn load(&self) -> Result<BoardState, String>;
    fn save(&self, board: &BoardState) -> Result<(), String>;
    fn reset(&self) -> Result<BoardState, String>;
}

pub struct JsonBoardStore<'a> {
    store: &'a Store,
}

impl<'a> JsonBoardStore<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    fn path(&self) -> std::path::PathBuf {
        self.store.layout().root().join(STORAGE_FILE)
    }
}

impl<'a> BoardStore for JsonBoardStore<'a> {
    fn load(&self) -> Result<BoardState, String> {
        let path = self.path();
        if !path.exists() {
            return Ok(BoardState::default_seed());
        }
        let bytes = std::fs::read(&path).map_err(|e| format!("read {path:?}: {e}"))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("parse {path:?}: {e}"))
    }

    fn save(&self, board: &BoardState) -> Result<(), String> {
        let path = self.path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
        }
        let json = serde_json::to_string_pretty(board).map_err(|e| format!("encode: {e}"))?;
        std::fs::write(&path, json).map_err(|e| format!("write {path:?}: {e}"))
    }

    fn reset(&self) -> Result<BoardState, String> {
        let mut board = BoardState::default_seed();
        board.updated_at = current_time_unix_ms();
        self.save(&board)?;
        Ok(board)
    }
}

fn build_store<'a>(store: &'a Store) -> Box<dyn BoardStore + 'a> {
    Box::new(JsonBoardStore::new(store))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveBoardPayload {
    pub board: BoardState,
}

#[tauri::command]
pub fn list_boards(state: State<'_, AppState>) -> Result<Vec<BoardState>, String> {
    let store = state.store();
    let board = build_store(store).load()?;
    Ok(vec![board])
}

#[tauri::command]
pub fn load_board(state: State<'_, AppState>) -> Result<BoardState, String> {
    let store = state.store();
    build_store(store).load()
}

#[tauri::command]
pub fn save_board(
    state: State<'_, AppState>,
    payload: SaveBoardPayload,
) -> Result<BoardState, String> {
    let store = state.store();
    let mut board = payload.board;
    board.updated_at = current_time_unix_ms();
    build_store(store).save(&board)?;
    Ok(board)
}

#[tauri::command]
pub fn reset_board(state: State<'_, AppState>) -> Result<BoardState, String> {
    let store = state.store();
    build_store(store).reset()
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
    fn default_seed_has_three_columns() {
        let b = BoardState::default_seed();
        assert_eq!(b.columns.len(), 3);
        assert_eq!(b.columns[0].id, "todo");
    }

    #[test]
    fn load_returns_seed_when_file_missing() {
        let tmp = tempdir().unwrap();
        let store = store_in(tmp.path());
        let bs = JsonBoardStore::new(&store);
        let board = bs.load().unwrap();
        assert_eq!(board.columns.len(), 3);
    }

    #[test]
    fn save_then_load_round_trips_a_card() {
        let tmp = tempdir().unwrap();
        let store = store_in(tmp.path());
        let bs = JsonBoardStore::new(&store);
        let mut board = BoardState::default_seed();
        board.columns[0].cards.push(Card {
            id: "c1".into(),
            title: "ship it".into(),
            body: String::new(),
            column_id: "todo".into(),
            ordinal: 0,
            updated_at: 1,
        });
        bs.save(&board).unwrap();
        let back = bs.load().unwrap();
        assert_eq!(back.columns[0].cards.len(), 1);
        assert_eq!(back.columns[0].cards[0].title, "ship it");
    }

    #[test]
    fn reset_clears_cards() {
        let tmp = tempdir().unwrap();
        let store = store_in(tmp.path());
        let bs = JsonBoardStore::new(&store);
        let mut board = BoardState::default_seed();
        board.columns[0].cards.push(Card {
            id: "c1".into(),
            title: "x".into(),
            body: String::new(),
            column_id: "todo".into(),
            ordinal: 0,
            updated_at: 1,
        });
        bs.save(&board).unwrap();
        let reset = bs.reset().unwrap();
        assert!(reset.columns.iter().all(|c| c.cards.is_empty()));
    }
}
