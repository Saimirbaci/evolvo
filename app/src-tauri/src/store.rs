use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::fsutil;
use crate::types::{FeedbackRecord, LineageJobRecord};

const WORKSPACE_DIR_NAME: &str = "evolvo_workspace";
const FEEDBACK_DIR: &str = "feedback";
const SANDBOX_JOBS_DIR: &str = "lineage_jobs";
const ATTACHMENTS_DIR: &str = "attachments";
const ITERATION_FILE: &str = "iteration.json";
/// Sibling directory (inside each entity dir) that unparseable JSON files
/// get moved into instead of being silently skipped. Not a `.json` file
/// itself, so `list_json_entities`'s extension filter naturally excludes
/// it from being walked as an entity — see the `.corrupted` dir doesn't
/// leak back into listings" test.
const CORRUPTED_DIR: &str = ".corrupted";

/// Hard ceiling on a single attachment's decoded byte size (canvas
/// screenshot, one pasted image, or the voice recording). Enforced again
/// here as defense in depth even though `commands::submit_feedback`
/// rejects oversized payloads before decoding — nothing should reach
/// `Store` without going through this check.
pub const MAX_ATTACHMENT_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug)]
pub enum StoreError {
    Io(io::Error),
    Serde(serde_json::Error),
    Other(String),
    AttachmentTooLarge {
        filename: String,
        size: usize,
        limit: usize,
    },
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Serde(e) => write!(f, "serde: {e}"),
            Self::Other(msg) => write!(f, "{msg}"),
            Self::AttachmentTooLarge {
                filename,
                size,
                limit,
            } => write!(
                f,
                "attachment {filename} is {size} bytes, which exceeds the {limit}-byte limit"
            ),
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
            fsutil::create_dir_all_private(&dir)?;
            // Best-effort: a directory created by an older Evolvo build
            // (before 0700 hardening landed) keeps whatever permissive
            // mode it already has — `create_dir_all` is a no-op on an
            // existing dir, so it never gets the tightened mode without
            // this explicit chmod.
            fsutil::tighten_permissions_best_effort(&dir);
        }
        // Sweep `.tmp` debris `write_atomic` can leave behind if the
        // process crashed between `fs::write(&tmp, ..)` and the rename.
        // These never match the `.json` extension filter, so they're
        // otherwise invisible clutter that accumulates forever.
        sweep_tmp_files(&self.layout.feedback_dir());
        sweep_tmp_files(&self.layout.lineage_jobs_dir());
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
        write_atomic(&self.feedback_path(&rec.id), json.as_bytes())?;
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
        write_atomic(&self.lineage_job_path(&rec.id), json.as_bytes())?;
        // Persistence remains the source of truth. Publishing happens only
        // after the atomic write succeeds and is deliberately best-effort:
        // a closed window must never turn a valid lineage transition into a
        // failed mutation. The initial UI snapshot/reconciliation catches up
        // when no listener is present.
        crate::lineage_events::publish_job_upserted(self.layout.root(), rec);
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

    /// Total number of `.corrupted/` entries across the feedback and
    /// lineage-jobs directories. Computed on demand (rather than threaded
    /// through `list_feedback`/`list_lineage_jobs`, which would change
    /// their signatures and every call site) for `app_health` to surface
    /// as `AppHealth::corrupted_entity_count`. Best-effort: an unreadable
    /// `.corrupted` dir counts as zero rather than failing the health
    /// check.
    pub fn count_quarantined_entities(&self) -> usize {
        count_entries(&self.layout.feedback_dir().join(CORRUPTED_DIR))
            + count_entries(&self.layout.lineage_jobs_dir().join(CORRUPTED_DIR))
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
        write_atomic(&path, serde_json::to_string_pretty(&body)?.as_bytes())?;
        Ok(current)
    }

    pub fn save_attachment(
        &self,
        feedback_id: &str,
        filename: &str,
        bytes: &[u8],
    ) -> Result<String, StoreError> {
        if bytes.len() > MAX_ATTACHMENT_BYTES {
            return Err(StoreError::AttachmentTooLarge {
                filename: filename.to_string(),
                size: bytes.len(),
                limit: MAX_ATTACHMENT_BYTES,
            });
        }

        let dir = self.layout.attachments_dir(feedback_id);
        fsutil::create_dir_all_private(&dir)?;

        let safe = sanitise_filename(filename);
        if safe.is_empty() {
            return Err("attachment filename cannot be empty".into());
        }
        let path = dir.join(&safe);
        write_atomic(&path, bytes)?;
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

/// Write `bytes` to `path` atomically. Thin wrapper over `fsutil::write_atomic`
/// that maps `io::Error` into `StoreError` — kept so existing call sites in
/// this file don't need `.map_err`.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    fsutil::write_atomic(path, bytes)?;
    Ok(())
}

/// List every well-formed `T` in `dir`'s `*.json` files. A file that fails
/// to parse is moved into `dir/.corrupted/` (preserving its filename, or
/// appending a unix-ms suffix on a collision) rather than silently skipped
/// — a corrupt record used to just vanish from the UI with no trace beyond
/// an `eprintln!`; quarantining keeps the bytes recoverable and lets
/// `Store::count_quarantined_entities` surface a non-zero count via
/// `AppHealth`. `.corrupted` itself has no `.json` extension, so it's
/// naturally excluded from being walked as an entity on the next pass.
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
                eprintln!(
                    "quarantining unreadable entity at {}: {err}",
                    path.display()
                );
                quarantine_file(dir, &path);
            }
        }
    }
    Ok(items)
}

/// Move `path` into `dir/.corrupted/`, keeping the original filename
/// unless that name is already taken there, in which case a unix-ms
/// suffix is appended so a repeated corruption of the same id doesn't
/// clobber an earlier quarantined copy. Best-effort: if the move itself
/// fails (e.g. permissions), logs and leaves the original file in place
/// rather than losing it.
fn quarantine_file(dir: &Path, path: &Path) {
    let corrupted_dir = dir.join(CORRUPTED_DIR);
    if let Err(err) = fs::create_dir_all(&corrupted_dir) {
        eprintln!(
            "failed to create quarantine dir {}: {err}",
            corrupted_dir.display()
        );
        return;
    }
    let Some(file_name) = path.file_name() else {
        return;
    };
    let mut dest = corrupted_dir.join(file_name);
    if dest.exists() {
        let now = crate::types::current_time_unix_ms();
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("entity");
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("json");
        dest = corrupted_dir.join(format!("{stem}.{now}.{ext}"));
    }
    if let Err(err) = fs::rename(path, &dest) {
        eprintln!(
            "failed to quarantine {} to {}: {err}",
            path.display(),
            dest.display()
        );
    }
}

/// Count regular files directly inside `dir` (non-recursive). Returns `0`
/// for a missing directory rather than erroring — the quarantine dir
/// legitimately doesn't exist until the first corrupt file shows up.
fn count_entries(dir: &Path) -> usize {
    if !dir.exists() {
        return 0;
    }
    fs::read_dir(dir)
        .map(|entries| entries.flatten().filter(|e| e.path().is_file()).count())
        .unwrap_or(0)
}

/// Remove leftover `*.tmp` files from a `write_atomic` that crashed
/// between the write and the rename. Called from `init_workspace` so
/// every store construction sweeps debris rather than letting it
/// accumulate silently. Best-effort — an unreadable directory or a
/// removal that fails (e.g. permissions) is logged, not propagated,
/// since this must never block startup.
fn sweep_tmp_files(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("tmp") {
            if let Err(err) = fs::remove_file(&path) {
                eprintln!("failed to sweep stale tmp file {}: {err}", path.display());
            }
        }
    }
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
    if let Ok(explicit) = std::env::var("EVOLVO_WORKSPACE_ROOT") {
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
    use crate::types::{AgentKind, FeedbackStatus, FeedbackType, LineageJobStatus};
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
            screens: vec![],
            voice_filename: None,
            voice_transcript: None,
            window_width: 100,
            window_height: 100,
            created_at_unix_ms: 10,
            updated_at_unix_ms: 10,
            lineage_job_id: None,
            local_delegate: None,
            revision: Default::default(),
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
    fn save_attachment_rejects_oversized_bytes() {
        let temp = tempdir().unwrap();
        let store = Store::new(temp.path().to_path_buf());
        store.init_workspace().unwrap();

        let oversized = vec![0u8; MAX_ATTACHMENT_BYTES + 1];
        let err = store
            .save_attachment("fb-1", "huge.png", &oversized)
            .unwrap_err();
        match err {
            StoreError::AttachmentTooLarge { filename, size, .. } => {
                assert_eq!(filename, "huge.png");
                assert_eq!(size, MAX_ATTACHMENT_BYTES + 1);
            }
            other => panic!("expected AttachmentTooLarge, got {other}"),
        }
        // Nothing should have been written to disk.
        assert!(store.read_attachment("fb-1", "huge.png").unwrap().is_none());
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
            freedom: crate::types::FreedomLevel::Auto,
            stages: Vec::new(),
            agent: AgentKind::ClaudeCode,
            pause_after_plan: false,
            plan_approved_at_unix_ms: None,
            base_branch: None,
            promoted_at_unix_ms: None,
            promoted_commit: None,
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

        // The corrupt file is quarantined, not deleted, and no longer
        // shows up on subsequent listings (it's not a `.json` sibling of
        // the entity dir anymore).
        let quarantined = store
            .layout()
            .feedback_dir()
            .join(".corrupted")
            .join("bad.json");
        assert!(quarantined.exists());
        assert!(!store.layout().feedback_dir().join("bad.json").exists());
        let all_again = store.list_feedback().unwrap();
        assert_eq!(all_again.len(), 1);
    }

    #[test]
    fn quarantine_count_is_reported_and_survives_collisions() {
        let temp = tempdir().unwrap();
        let store = Store::new(temp.path().to_path_buf());
        store.init_workspace().unwrap();

        store.save_feedback(&sample("good")).unwrap();
        fs::write(store.layout().feedback_dir().join("bad1.json"), b"{oops").unwrap();
        store.list_feedback().unwrap();
        assert_eq!(store.count_quarantined_entities(), 1);

        // A second corruption of a file with the same name must not
        // clobber the first quarantined copy.
        fs::write(store.layout().feedback_dir().join("bad1.json"), b"{oops2").unwrap();
        store.list_feedback().unwrap();
        assert_eq!(store.count_quarantined_entities(), 2);

        fs::write(
            store.layout().lineage_jobs_dir().join("badjob.json"),
            b"not json",
        )
        .unwrap();
        store.list_lineage_jobs().unwrap();
        assert_eq!(store.count_quarantined_entities(), 3);
    }

    #[test]
    fn init_workspace_sweeps_leftover_tmp_files() {
        let temp = tempdir().unwrap();
        let store = Store::new(temp.path().to_path_buf());
        store.init_workspace().unwrap();

        // Simulate a crash between `fs::write(&tmp, ..)` and the rename.
        fs::write(store.layout().feedback_dir().join("fb-1.json.tmp"), b"{}").unwrap();
        fs::write(
            store.layout().lineage_jobs_dir().join("job-1.json.tmp"),
            b"{}",
        )
        .unwrap();

        store.init_workspace().unwrap();

        assert!(!store.layout().feedback_dir().join("fb-1.json.tmp").exists());
        assert!(!store
            .layout()
            .lineage_jobs_dir()
            .join("job-1.json.tmp")
            .exists());
    }

    #[cfg(unix)]
    #[test]
    fn init_workspace_creates_dirs_with_0700() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempdir().unwrap();
        let store = Store::new(temp.path().to_path_buf());
        store.init_workspace().unwrap();

        for dir in store.layout().directories() {
            let mode = fs::metadata(&dir).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700, "{} was not 0700", dir.display());
        }
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
