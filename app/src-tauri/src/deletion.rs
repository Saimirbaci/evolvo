//! Deleting a lineage job and everything that belongs to it.
//!
//! The Lineage list's per-card Delete removes, for one job:
//! - the version Evolvo launched for it (stopped),
//! - its git worktree and its `lineage/<job>` branch in the source repo,
//! - its job workspace (`lineage_workspaces/<job>/`: logs, plan, inputs,
//!   run spec, iteration run workspace),
//! - the feedback record that created it and that feedback's attachments,
//! - the job record itself — **last**, so a failure part-way leaves the job
//!   listed and the delete can simply be retried.
//!
//! Kept on purpose: shared clones under `external_repos/` (other jobs and
//! the remembered app → repo mapping use them) and commits already merged
//! into a source repo by promotion.
//!
//! Refused while an agent or merge is running on the job, while its source
//! repo is still being resolved, or when another job builds on this one
//! (its worktree is that job's source repo) — deleting would pull the
//! ground from under them.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::store::Store;
use crate::types::LineageJobStatus;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeleteSummary {
    pub job_id: String,
    /// Human-readable list of what was removed.
    pub removed: Vec<String>,
}

/// Same rule as store filenames: ids that reach a path must be plain.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('.')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

pub fn delete_lineage_job(store: &Store, job_id: &str) -> Result<DeleteSummary, String> {
    if !valid_id(job_id) {
        return Err(format!("invalid lineage job id: {job_id:?}"));
    }
    let lock = crate::lineage::job_write_lock(job_id);
    let _guard = lock.lock().unwrap_or_else(|p| p.into_inner());

    let job = store
        .load_lineage_job(job_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("lineage job not found: {job_id}"))?;

    if matches!(
        job.status,
        LineageJobStatus::Implementing | LineageJobStatus::Triaging | LineageJobStatus::Merging
    ) {
        return Err(
            "an agent or merge is still running on this job — wait for it to finish or fail, then delete it"
                .into(),
        );
    }
    if crate::external::is_resolving(&job.feedback_id) {
        return Err(
            "Evolvo is still finding or cloning this app's source — try again when that finishes"
                .into(),
        );
    }

    let worktree = job.worktree_path.as_deref().map(PathBuf::from);
    let dependents: Vec<String> = store
        .list_lineage_jobs()
        .unwrap_or_default()
        .into_iter()
        .filter(|other| other.id != job.id)
        .filter(|other| {
            let forked_from_here = match (&worktree, other.source_repo.as_deref()) {
                (Some(w), Some(src)) => Path::new(src) == w.as_path(),
                _ => false,
            };
            let based_on_here = store
                .load_feedback(&other.feedback_id)
                .ok()
                .flatten()
                .and_then(|f| f.target_app)
                .and_then(|t| t.based_on_job)
                .is_some_and(|b| b == job.id);
            forked_from_here || based_on_here
        })
        .map(|other| other.id)
        .collect();
    if !dependents.is_empty() {
        return Err(format!(
            "{} build{} on this job ({}) — delete {} first",
            if dependents.len() == 1 {
                "Another job"
            } else {
                "Other jobs"
            },
            if dependents.len() == 1 { "s" } else { "" },
            dependents.join(", "),
            if dependents.len() == 1 { "it" } else { "them" },
        ));
    }

    let mut removed = Vec::new();
    let root = store.layout().root().to_path_buf();

    if crate::apprun::stop(&job.id) {
        removed.push("stopped the running version".to_string());
    }

    // Worktree + branch + job workspace. The worktree is removed through
    // the repo it was forked from (another app's repo for external jobs).
    let job_dir = crate::runner::job_workspace_dir(&root, &job.id);
    let source = job
        .source_repo
        .as_deref()
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .or_else(crate::runner::resolve_source_repo);
    let had_worktree = worktree.as_deref().is_some_and(Path::exists);
    match source {
        Some(src) if job.branch_name.is_some() || had_worktree => {
            crate::runner::cleanup_previous_run(&src, &root, &job.id).map_err(|e| e.to_string())?;
            removed.push(format!(
                "worktree and branch {}",
                job.branch_name
                    .clone()
                    .unwrap_or_else(|| crate::runner::branch_name(&job.id))
            ));
        }
        _ => {}
    }
    if job_dir.exists() {
        std::fs::remove_dir_all(&job_dir).map_err(|e| format!("remove job workspace: {e}"))?;
    }
    removed.push("job workspace (logs, plan, inputs)".to_string());

    // The feedback that created this job, unless it now belongs to another.
    if let Ok(Some(fb)) = store.load_feedback(&job.feedback_id) {
        let owned = fb.lineage_job_id.as_deref().is_none_or(|j| j == job.id);
        if owned {
            if let Some(parent_id) = fb.revision.parent_feedback_id.as_deref() {
                if let Ok(Some(mut parent)) = store.load_feedback(parent_id) {
                    if parent.revision.superseded_by_feedback_id.as_deref() == Some(fb.id.as_str())
                    {
                        parent.revision.superseded_by_feedback_id = None;
                        let _ = store.save_feedback(&parent);
                    }
                }
            }
            store
                .delete_feedback(&fb.id)
                .map_err(|e| format!("remove feedback: {e}"))?;
            removed.push(format!("feedback {} and its attachments", fb.id));
        }
    }

    // Record last: everything above can be retried while it still exists.
    store
        .delete_lineage_job(&job.id)
        .map_err(|e| format!("remove job record: {e}"))?;
    removed.push("job record".to_string());

    crate::lineage_events::publish_snapshot_invalidated();
    crate::tray::request_refresh();
    Ok(DeleteSummary {
        job_id: job.id,
        removed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lineage::LineageEngine;
    use crate::types::{AgentKind, FeedbackRecord, TargetApp};
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        assert!(Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .status()
            .unwrap()
            .success());
    }

    fn feedback(id: &str, repo: &Path) -> FeedbackRecord {
        let mut fb: FeedbackRecord = serde_json::from_value(serde_json::json!({
            "id": id, "feedbackType": "bug", "status": "new", "pageRoute": "/",
            "feedbackText": "x", "screenshotFilename": null, "voiceFilename": null,
            "voiceTranscript": null, "windowWidth": 1, "windowHeight": 1,
            "createdAtUnixMs": 1, "updatedAtUnixMs": 1, "lineageJobId": null
        }))
        .unwrap();
        fb.target_app = Some(TargetApp {
            name: "App".into(),
            repo_path: Some(repo.display().to_string()),
            repo_status: "resolved".into(),
            ..Default::default()
        });
        fb
    }

    /// An external-app job with a real worktree + branch in `repo`.
    fn prepared_job(store: &Store, repo: &Path, fb_id: &str) -> crate::types::LineageJobRecord {
        let engine = LineageEngine::new(store);
        let mut fb = feedback(fb_id, repo);
        store.save_feedback(&fb).unwrap();
        store.save_attachment(fb_id, "canvas.png", b"png").unwrap();
        let job = engine
            .enqueue_job_for_feedback(&mut fb, AgentKind::ClaudeCode)
            .unwrap();
        let fb = store.load_feedback(fb_id).unwrap().unwrap();
        let prepared = crate::runner::prepare_run(store, &job, &fb, AgentKind::ClaudeCode).unwrap();
        engine
            .set_run_artifacts(
                &job.id,
                prepared.worktree.display().to_string(),
                prepared.branch.clone(),
                prepared.log_file.display().to_string(),
                prepared.source_repo.display().to_string(),
                prepared.base_branch.clone(),
            )
            .unwrap();
        engine
            .force_status(&job.id, LineageJobStatus::BuildReady)
            .unwrap()
    }

    fn repo(dir: &Path) -> PathBuf {
        let repo = dir.join("app-src");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("README.md"), "x\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "init"]);
        repo
    }

    fn branch_exists(repo: &Path, branch: &str) -> bool {
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["rev-parse", "--verify", "--quiet", branch])
            .status()
            .unwrap()
            .success()
    }

    #[test]
    fn deletes_worktree_branch_workspace_feedback_and_record() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = repo(tmp.path());
        let store = Store::new(tmp.path().join("ws"));
        store.init_workspace().unwrap();
        let job = prepared_job(&store, &repo, "fb-del");
        let worktree = PathBuf::from(job.worktree_path.clone().unwrap());
        let branch = job.branch_name.clone().unwrap();
        assert!(worktree.exists() && branch_exists(&repo, &branch));

        let summary = delete_lineage_job(&store, &job.id).unwrap();
        assert!(summary.removed.iter().any(|r| r.contains(&branch)));
        assert!(!worktree.exists());
        assert!(!branch_exists(&repo, &branch));
        assert!(!crate::runner::job_workspace_dir(store.layout().root(), &job.id).exists());
        assert!(store.load_feedback("fb-del").unwrap().is_none());
        assert!(!store.layout().attachments_dir("fb-del").exists());
        assert!(store.load_lineage_job(&job.id).unwrap().is_none());
        // The source repo itself is untouched.
        assert!(repo.join("README.md").exists());
    }

    #[test]
    fn refuses_running_jobs_bad_ids_and_jobs_others_build_on() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = repo(tmp.path());
        let store = Store::new(tmp.path().join("ws"));
        store.init_workspace().unwrap();
        assert!(delete_lineage_job(&store, "../etc").is_err());

        let base = prepared_job(&store, &repo, "fb-base");
        // A follow-up job forked from the base job's worktree.
        let base_wt = PathBuf::from(base.worktree_path.clone().unwrap());
        let child = prepared_job(&store, &base_wt, "fb-child");
        let err = delete_lineage_job(&store, &base.id).unwrap_err();
        assert!(err.contains(&child.id), "{err}");

        LineageEngine::new(&store)
            .force_status(&child.id, LineageJobStatus::Implementing)
            .unwrap();
        let err = delete_lineage_job(&store, &child.id).unwrap_err();
        assert!(err.contains("still running"), "{err}");

        // Once the follow-up is gone, the base can be deleted.
        LineageEngine::new(&store)
            .force_status(&child.id, LineageJobStatus::Failed)
            .unwrap();
        delete_lineage_job(&store, &child.id).unwrap();
        delete_lineage_job(&store, &base.id).unwrap();
        assert!(store.list_lineage_jobs().unwrap().is_empty());
    }

    #[test]
    fn pending_job_without_a_run_deletes_cleanly_and_unlinks_revision_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("ws"));
        store.init_workspace().unwrap();
        let engine = LineageEngine::new(&store);

        let mut parent = feedback("fb-parent", tmp.path());
        parent.target_app = None;
        parent.revision.superseded_by_feedback_id = Some("fb-rev".into());
        store.save_feedback(&parent).unwrap();
        let mut rev = feedback("fb-rev", tmp.path());
        rev.target_app = None;
        rev.revision.parent_feedback_id = Some("fb-parent".into());
        rev.revision.number = 2;
        store.save_feedback(&rev).unwrap();
        let job = engine
            .enqueue_job_for_feedback(&mut rev, AgentKind::ClaudeCode)
            .unwrap();

        let summary = delete_lineage_job(&store, &job.id).unwrap();
        assert!(summary.removed.iter().any(|r| r.contains("fb-rev")));
        assert!(store.load_lineage_job(&job.id).unwrap().is_none());
        let parent = store.load_feedback("fb-parent").unwrap().unwrap();
        assert_eq!(parent.revision.superseded_by_feedback_id, None);
    }
}
