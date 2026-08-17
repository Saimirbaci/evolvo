//! Event-driven projection of Evolvo's durable lineage state.
//!
//! Lineage JSON and per-job workspaces remain authoritative. This module is
//! only a notification layer: successful state writes publish complete job
//! snapshots, while a scoped filesystem watcher coalesces files written by
//! external coding-agent processes into semantic artifact-change events.
//! Missing listeners or event-delivery failures never affect persistence.

use std::collections::{BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, OnceLock, RwLock};
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecursiveMode, Watcher};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::runner;
use crate::store::Store;
use crate::types::{current_time_unix_ms, LineageJobRecord};

pub const JOB_UPSERTED_EVENT: &str = "lineage://job-upserted";
pub const ARTIFACTS_CHANGED_EVENT: &str = "lineage://artifacts-changed";
pub const SNAPSHOT_INVALIDATED_EVENT: &str = "lineage://snapshot-invalidated";

const DEBOUNCE_WINDOW: Duration = Duration::from_millis(750);

/// Semantic kinds let the UI refresh only the artifact-backed panels that
/// can actually have changed. Unknown worktree writes intentionally collapse
/// to `workspace`, preserving room for a future Git/file summary panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Plan,
    Delegation,
    Log,
    Workspace,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LineageArtifactsChanged {
    pub job_id: String,
    pub changed_at_unix_ms: u64,
    pub kinds: Vec<ArtifactKind>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LineageJobUpserted {
    pub revision: u64,
    pub job: LineageJobRecord,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotInvalidated {
    changed_at_unix_ms: u64,
}

/// Domain-facing event sink. Keeping Tauri behind this boundary lets the
/// store, runner, promotion code, and tests retain their current ownership
/// model instead of threading `AppHandle` through every lineage engine.
pub trait LineageEventSink: Send + Sync + 'static {
    fn job_upserted(&self, workspace_root: &Path, event: &LineageJobUpserted);
    fn artifacts_changed(&self, event: &LineageArtifactsChanged);
    fn snapshot_invalidated(&self);
}

fn sink_slot() -> &'static RwLock<Option<Arc<dyn LineageEventSink>>> {
    static SLOT: OnceLock<RwLock<Option<Arc<dyn LineageEventSink>>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(None))
}

pub fn install_tauri_sink(app: AppHandle, workspace_root: PathBuf) {
    let mut slot = sink_slot().write().unwrap_or_else(|p| p.into_inner());
    *slot = Some(Arc::new(TauriLineageEventSink {
        app,
        workspace_root,
    }));
}

fn with_sink(f: impl FnOnce(&dyn LineageEventSink)) {
    let slot = sink_slot().read().unwrap_or_else(|p| p.into_inner());
    if let Some(sink) = slot.as_deref() {
        f(sink);
    }
}

pub fn publish_job_upserted(workspace_root: &Path, job: &LineageJobRecord) {
    static NEXT_REVISION: AtomicU64 = AtomicU64::new(1);
    let event = LineageJobUpserted {
        revision: NEXT_REVISION.fetch_add(1, Ordering::Relaxed),
        job: job.clone(),
    };
    with_sink(|sink| sink.job_upserted(workspace_root, &event));
}

fn publish_artifacts_changed(job_id: String, kinds: BTreeSet<ArtifactKind>) {
    if kinds.is_empty() {
        return;
    }
    let event = LineageArtifactsChanged {
        job_id,
        changed_at_unix_ms: current_time_unix_ms(),
        kinds: kinds.into_iter().collect(),
    };
    with_sink(|sink| sink.artifacts_changed(&event));
}

fn publish_snapshot_invalidated() {
    with_sink(LineageEventSink::snapshot_invalidated);
}

struct TauriLineageEventSink {
    app: AppHandle,
    workspace_root: PathBuf,
}

impl LineageEventSink for TauriLineageEventSink {
    fn job_upserted(&self, workspace_root: &Path, event: &LineageJobUpserted) {
        // Bundle imports write into a different Evolvo workspace. Those
        // records belong to the forked app and must never appear in the host
        // app's live Lineage projection.
        if workspace_root == self.workspace_root {
            let _ = self.app.emit(JOB_UPSERTED_EVENT, event);
        }
    }

    fn artifacts_changed(&self, event: &LineageArtifactsChanged) {
        let _ = self.app.emit(ARTIFACTS_CHANGED_EVENT, event);
    }

    fn snapshot_invalidated(&self) {
        let _ = self.app.emit(
            SNAPSHOT_INVALIDATED_EVENT,
            SnapshotInvalidated {
                changed_at_unix_ms: current_time_unix_ms(),
            },
        );
    }
}

#[derive(Debug)]
enum PendingChange {
    JobRecord(String),
    Artifact(String, ArtifactKind),
}

/// Watch the Evolvo workspace recursively on a dedicated thread. Native
/// watcher callbacks are intentionally reduced to job-scoped semantic
/// changes and flushed at most once per debounce window, so formatters and
/// package managers cannot cause a UI event storm.
pub fn start_workspace_watcher(workspace_root: PathBuf) -> Result<(), String> {
    std::fs::create_dir_all(&workspace_root).map_err(|e| {
        format!(
            "create lineage watcher root {}: {e}",
            workspace_root.display()
        )
    })?;

    std::thread::Builder::new()
        .name("evolvo-lineage-watch".into())
        .spawn(move || run_workspace_watcher(workspace_root))
        .map_err(|e| format!("spawn lineage filesystem watcher: {e}"))?;
    Ok(())
}

fn run_workspace_watcher(workspace_root: PathBuf) {
    let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
    let mut watcher = match notify::recommended_watcher(tx) {
        Ok(watcher) => watcher,
        Err(e) => {
            eprintln!("evolvo: lineage filesystem watcher unavailable: {e}");
            return;
        }
    };
    if let Err(e) = watcher.watch(&workspace_root, RecursiveMode::Recursive) {
        eprintln!(
            "evolvo: failed to watch lineage workspace {}: {e}",
            workspace_root.display()
        );
        return;
    }

    let store = Store::new(workspace_root.clone());
    let mut pending_jobs = BTreeSet::new();
    let mut pending_artifacts: HashMap<String, BTreeSet<ArtifactKind>> = HashMap::new();
    let mut flush_at: Option<Instant> = None;

    loop {
        let timeout = flush_at
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
            .unwrap_or(Duration::from_secs(60));
        match rx.recv_timeout(timeout) {
            Ok(Ok(event)) => {
                if event_is_relevant(&event.kind) {
                    for path in event.paths {
                        if let Some(change) = classify_change(&workspace_root, &path) {
                            match change {
                                PendingChange::JobRecord(id) => {
                                    pending_jobs.insert(id);
                                }
                                PendingChange::Artifact(id, kind) => {
                                    pending_artifacts.entry(id).or_default().insert(kind);
                                }
                            }
                            if flush_at.is_none() {
                                flush_at = Some(Instant::now() + DEBOUNCE_WINDOW);
                            }
                        }
                    }
                }
            }
            Ok(Err(e)) => eprintln!("evolvo: lineage filesystem watcher error: {e}"),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if flush_at.is_none() {
                    continue;
                }
                for job_id in std::mem::take(&mut pending_jobs) {
                    match store.load_lineage_job(&job_id) {
                        Ok(Some(job)) => publish_job_upserted(&workspace_root, &job),
                        Ok(None) | Err(_) => publish_snapshot_invalidated(),
                    }
                }
                for (job_id, kinds) in std::mem::take(&mut pending_artifacts) {
                    publish_artifacts_changed(job_id, kinds);
                }
                flush_at = None;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn event_is_relevant(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) | EventKind::Any
    )
}

fn classify_change(root: &Path, path: &Path) -> Option<PendingChange> {
    let relative = path.strip_prefix(root).ok()?;
    let mut parts = relative.components();
    let first = normal_component(parts.next()?)?;

    if first == "lineage_jobs" {
        let filename = normal_component(parts.next()?)?;
        let job_id = filename.strip_suffix(".json")?;
        return (!job_id.is_empty()).then(|| PendingChange::JobRecord(job_id.to_string()));
    }

    if first != "lineage_workspaces" {
        return None;
    }
    let job_id = normal_component(parts.next()?)?.to_string();
    let remaining: Vec<&str> = parts.filter_map(normal_component).collect();
    if remaining.is_empty() || remaining.iter().any(|part| ignored_component(part)) {
        return None;
    }

    let filename = remaining.last().copied().unwrap_or_default();
    let kind = if filename == crate::plan::PLAN_FILENAME {
        ArtifactKind::Plan
    } else if filename == runner::PI_EVENTS_FILENAME {
        ArtifactKind::Delegation
    } else if filename.ends_with(".log") {
        ArtifactKind::Log
    } else {
        ArtifactKind::Workspace
    };
    Some(PendingChange::Artifact(job_id, kind))
}

fn normal_component(component: Component<'_>) -> Option<&str> {
    match component {
        Component::Normal(value) => value.to_str(),
        _ => None,
    }
}

fn ignored_component(component: &str) -> bool {
    matches!(
        component,
        ".git" | "node_modules" | "target" | "dist" | ".next" | ".turbo" | ".cache" | "coverage"
    ) || component.ends_with(".tmp")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_lineage_record_changes() {
        let root = Path::new("/workspace");
        let path = root.join("lineage_jobs/job-123.json");
        assert!(matches!(
            classify_change(root, &path),
            Some(PendingChange::JobRecord(id)) if id == "job-123"
        ));
    }

    #[test]
    fn classifies_agent_artifacts_and_ignores_build_noise() {
        let root = Path::new("/workspace");
        let plan = root.join("lineage_workspaces/job-7/plan.json");
        let delegation = root.join("lineage_workspaces/job-7/pi-events.jsonl");
        let source = root.join("lineage_workspaces/job-7/worktree/src/app.rs");
        let build = root.join("lineage_workspaces/job-7/worktree/target/debug/app");

        assert!(matches!(
            classify_change(root, &plan),
            Some(PendingChange::Artifact(id, ArtifactKind::Plan)) if id == "job-7"
        ));
        assert!(matches!(
            classify_change(root, &delegation),
            Some(PendingChange::Artifact(id, ArtifactKind::Delegation)) if id == "job-7"
        ));
        assert!(matches!(
            classify_change(root, &source),
            Some(PendingChange::Artifact(id, ArtifactKind::Workspace)) if id == "job-7"
        ));
        assert!(classify_change(root, &build).is_none());
    }
}
