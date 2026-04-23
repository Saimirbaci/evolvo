use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::types::{FeedbackRecord, LineageJobRecord};

const WORKSPACE_DIR_NAME: &str = "evolvo_workspace";
const FEEDBACK_DIR: &str = "feedback";
const SANDBOX_JOBS_DIR: &str = "lineage_jobs";
const ATTACHMENTS_DIR: &str = "attachments";
const ITERATION_FILE: &str = "iteration.json";

#[derive(Debug)]
pub enum StoreError {
    Io(io::Error),
    Serde(serde_json::Error),
    Other(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Serde(e) => write!(f, "serde: {e}"),
            Self::Other(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<io::Error> for StoreError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(e: serde_json::Error) -> Self {
        Self::Serde(e)
    }
}

impl From<String> for StoreError {
    fn from(e: String) -> Self {
        Self::Other(e)
    }
}

impl From<&str> for StoreError {
    fn from(e: &str) -> Self {
        Self::Other(e.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct WorkspaceLayout {
    root: PathBuf,
}

impl WorkspaceLayout {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn feedback_dir(&self) -> PathBuf {
        self.root.join(FEEDBACK_DIR)
    }

    pub fn lineage_jobs_dir(&self) -> PathBuf {
        self.root.join(SANDBOX_JOBS_DIR)
    }

    pub fn attachments_dir(&self, id: &str) -> PathBuf {
        self.root.join(ATTACHMENTS_DIR).join(id)
    }

    pub fn directories(&self) -> Vec<PathBuf> {
        vec![
            self.root.clone(),
            self.feedback_dir(),
            self.lineage_jobs_dir(),
            self.root.join(ATTACHMENTS_DIR),
        ]
    }
}

#[derive(Debug, Clone)]
pub struct Store {
    layout: WorkspaceLayout,
}

impl Store {
    pub fn new(root: PathBuf) -> Self {
        Self {
            layout: WorkspaceLayout::new(root),
        }
    }

    pub fn layout(&self) -> &WorkspaceLayout {
        &self.layout
    }

    pub fn init_workspace(&self) -> Result<(), StoreError> {
        for dir in self.layout.directories() {
            fs::create_dir_all(&dir)?;
        }
        Ok(())
    }

    fn feedback_path(&self, id: &str) -> PathBuf {
        self.layout.feedback_dir().join(format!("{id}.json"))
    }

    fn lineage_job_path(&self, id: &str) -> PathBuf {
        self.layout.lineage_jobs_dir().join(format!("{id}.json"))
    }

    pub fn save_feedback(&self, rec: &FeedbackRecord) -> Result<(), StoreError> {
        self.init_workspace()?;
        let json = serde_json::to_string_pretty(rec)?;
        fs::write(self.feedback_path(&rec.id), json)?;
        Ok(())
    }

    pub fn load_feedback(&self, id: &str) -> Result<Option<FeedbackRecord>, StoreError> {
        let path = self.feedback_path(id);
        if !path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&path)?;
        let rec: FeedbackRecord = serde_json::from_slice(&bytes)?;
        Ok(Some(rec))
    }

    pub fn list_feedback(&self) -> Result<Vec<FeedbackRecord>, StoreError> {
        list_json_entities::<FeedbackRecord>(&self.layout.feedback_dir())
    }

    pub fn delete_feedback(&self, id: &str) -> Result<bool, StoreError> {
        let path = self.feedback_path(id);
        if !path.exists() {
            return Ok(false);
        }
        fs::remove_file(&path)?;
        let _ = fs::remove_dir_all(self.layout.attachments_dir(id));
        Ok(true)
    }

    pub fn save_lineage_job(&self, rec: &LineageJobRecord) -> Result<(), StoreError> {
        self.init_workspace()?;
        let json = serde_json::to_string_pretty(rec)?;
        fs::write(self.lineage_job_path(&rec.id), json)?;
        Ok(())
    }

    pub fn load_lineage_job(&self, id: &str) -> Result<Option<LineageJobRecord>, StoreError> {
        let path = self.lineage_job_path(id);
        if !path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&path)?;
        let rec: LineageJobRecord = serde_json::from_slice(&bytes)?;
        Ok(Some(rec))
    }

    pub fn list_lineage_jobs(&self) -> Result<Vec<LineageJobRecord>, StoreError> {
        list_json_entities::<LineageJobRecord>(&self.layout.lineage_jobs_dir())
    }

    /// Allocate the next iteration number for this workspace. The counter is
    /// stored in `iteration.json` at the workspace root as
    /// `{"nextIteration": N}`. A fresh workspace returns `1` on first call
    /// and increments monotonically thereafter. Reads-modify-writes are not
    /// concurrency-safe — the pipeline serialises on the Tauri invoke thread,
    /// so that's fine for now; if that ever changes, wrap in a file lock.
    pub fn allocate_iteration(&self) -> Result<u32, StoreError> {
        self.init_workspace()?;
        let path = self.layout.root.join(ITERATION_FILE);
        let current: u32 = if path.exists() {
            let bytes = fs::read(&path)?;
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
            v.get("nextIteration")
                .and_then(|x| x.as_u64())
                .map(|n| n as u32)
                .unwrap_or(1)
        } else {
            1
        };
        let next = current.saturating_add(1);
        let body = serde_json::json!({ "nextIteration": next });
        fs::write(&path, serde_json::to_string_pretty(&body)?)?;
        Ok(current)
    }

    pub fn save_attachment(
        &self,
        feedback_id: &str,
        filename: &str,
        bytes: &[u8],
    ) -> Result<String, StoreError> {
        let dir = self.layout.attachments_dir(feedback_id);
        fs::create_dir_all(&dir)?;

        let safe = sanitise_filename(filename);
        if safe.is_empty() {
            return Err("attachment filename cannot be empty".into());
        }
        let path = dir.join(&safe);
        fs::write(&path, bytes)?;
        Ok(safe)
    }

    pub fn read_attachment(
        &self,
        feedback_id: &str,
        filename: &str,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        let safe = sanitise_filename(filename);
        if safe.is_empty() {
            return Ok(None);
        }
        let path = self.layout.attachments_dir(feedback_id).join(&safe);
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(fs::read(&path)?))
    }
}

fn list_json_entities<T: serde::de::DeserializeOwned>(dir: &Path) -> Result<Vec<T>, StoreError> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let entries = fs::read_dir(dir)?;
    let mut items = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        match fs::read(&path).and_then(|bytes| {
            serde_json::from_slice::<T>(&bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
        }) {
            Ok(rec) => items.push(rec),
            Err(err) => {
                eprintln!("skip unreadable entity at {}: {err}", path.display());
                continue;
            }
        }
    }
    Ok(items)
}

fn sanitise_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .collect();
    if cleaned.starts_with('.') {
        cleaned.trim_start_matches('.').to_string()
    } else {
        cleaned
    }
}

pub fn default_workspace_root() -> PathBuf {
    if let Ok(explicit) = std::env::var("NOIDE_WORKSPACE_ROOT") {
        return PathBuf::from(explicit);
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".evolvo").join(WORKSPACE_DIR_NAME);
    }
    PathBuf::from(WORKSPACE_DIR_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FeedbackStatus, FeedbackType, LineageJobStatus};
    use tempfile::tempdir;

    fn sample(id: &str) -> FeedbackRecord {
        FeedbackRecord {
            id: id.into(),
            feedback_type: FeedbackType::Bug,
            status: FeedbackStatus::New,
            page_route: "/".into(),
            feedback_text: "hello".into(),
            annotations: vec![],
            pasted_images: vec![],
            screenshot_filename: None,
            voice_filename: None,
            voice_transcript: None,
            window_width: 100,
            window_height: 100,
            created_at_unix_ms: 10,
            updated_at_unix_ms: 10,
            lineage_job_id: None,
        }
    }

    #[test]
    fn feedback_crud() {
        let temp = tempdir().unwrap();
        let store = Store::new(temp.path().to_path_buf());
        store.init_workspace().unwrap();

        store.save_feedback(&sample("a")).unwrap();
        store.save_feedback(&sample("b")).unwrap();
        let all = store.list_feedback().unwrap();
        assert_eq!(all.len(), 2);

        let back = store.load_feedback("a").unwrap().unwrap();
        assert_eq!(back.feedback_text, "hello");

        assert!(store.delete_feedback("a").unwrap());
        assert!(store.load_feedback("a").unwrap().is_none());
        assert!(!store.delete_feedback("a").unwrap());
    }

    #[test]
    fn attachments_are_lineageed_by_feedback_id() {
        let temp = tempdir().unwrap();
        let store = Store::new(temp.path().to_path_buf());
        store.init_workspace().unwrap();

        let name = store
            .save_attachment("fb-1", "screenshot.png", &[1, 2, 3])
            .unwrap();
        assert_eq!(name, "screenshot.png");
        assert_eq!(
            store.read_attachment("fb-1", "screenshot.png").unwrap(),
            Some(vec![1, 2, 3])
        );
        assert!(store
            .read_attachment("fb-2", "screenshot.png")
            .unwrap()
            .is_none());
    }

    #[test]
    fn lineage_job_crud() {
        let temp = tempdir().unwrap();
        let store = Store::new(temp.path().to_path_buf());
        store.init_workspace().unwrap();

        let job = LineageJobRecord {
            id: "job-1".into(),
            feedback_id: "fb-1".into(),
            title: "Fix".into(),
            summary: "Summary".into(),
            status: LineageJobStatus::Pending,
            notes: vec![],
            created_at_unix_ms: 0,
            updated_at_unix_ms: 0,
            worktree_path: None,
            branch_name: None,
            log_path: None,
            source_repo: None,
            iteration: 0,
        };
        store.save_lineage_job(&job).unwrap();
        assert_eq!(store.list_lineage_jobs().unwrap().len(), 1);
        let back = store.load_lineage_job("job-1").unwrap().unwrap();
        assert_eq!(back.status, LineageJobStatus::Pending);
    }

    #[test]
    fn list_skips_malformed_files() {
        let temp = tempdir().unwrap();
        let store = Store::new(temp.path().to_path_buf());
        store.init_workspace().unwrap();

        store.save_feedback(&sample("good")).unwrap();
        fs::write(
            store.layout().feedback_dir().join("bad.json"),
            b"not valid json",
        )
        .unwrap();

        let all = store.list_feedback().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "good");
    }

    #[test]
    fn allocate_iteration_starts_at_one_and_increments() {
        let temp = tempdir().unwrap();
        let store = Store::new(temp.path().to_path_buf());
        assert_eq!(store.allocate_iteration().unwrap(), 1);
        assert_eq!(store.allocate_iteration().unwrap(), 2);
        assert_eq!(store.allocate_iteration().unwrap(), 3);
    }

    #[test]
    fn sanitise_prevents_path_traversal() {
        assert_eq!(sanitise_filename("../etc/passwd"), "etcpasswd");
        assert_eq!(sanitise_filename("a b c.png"), "abc.png");
        assert_eq!(sanitise_filename("....png"), "png");
    }
}
