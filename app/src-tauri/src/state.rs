use std::path::PathBuf;
use std::sync::Mutex;

use crate::store::{default_workspace_root, Store};
use crate::types::current_time_unix_ms;

pub struct AppState {
    pub launched_at_unix_ms: u64,
    workspace_root: Mutex<PathBuf>,
}

impl AppState {
    pub fn new() -> Self {
        let root = default_workspace_root();
        if let Err(err) = std::fs::create_dir_all(&root) {
            eprintln!(
                "warning: failed to create workspace root {}: {err}",
                root.display()
            );
        }
        Self {
            launched_at_unix_ms: current_time_unix_ms(),
            workspace_root: Mutex::new(root),
        }
    }

    pub fn with_root(root: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&root);
        Self {
            launched_at_unix_ms: current_time_unix_ms(),
            workspace_root: Mutex::new(root),
        }
    }

    pub fn store(&self) -> Store {
        let root = self
            .workspace_root
            .lock()
            .expect("workspace root mutex poisoned")
            .clone();
        Store::new(root)
    }

    pub fn workspace_root_display(&self) -> String {
        self.workspace_root
            .lock()
            .expect("workspace root mutex poisoned")
            .display()
            .to_string()
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
