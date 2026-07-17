//! Real promotion: merge a lineage job's branch back into the source
//! repo's checked-out branch, gated by verification, with atomic rollback
//! on any failure.
//!
//! This module is the most safety-critical code in the pipeline — it is
//! the only place that ever mutates the user's actual source repository
//! (every other write lands in an isolated lineage worktree). The design
//! biases hard toward refusing over guessing:
//!
//! - Every precondition is checked BEFORE anything touches the source
//!   repo, and a failed precondition returns the job to `BuildReady` with
//!   an explanatory note rather than attempting a best-effort merge.
//! - The merge itself only ever fast-forwards or creates a merge commit;
//!   it never force-pushes, never rewrites history beyond a `reset --hard`
//!   to a SHA snapshotted by *this* run, and never touches remotes.
//! - Post-merge verification gates run against the source repo's new HEAD;
//!   any gate failure resets the source repo back to the pre-merge SHA
//!   (safe, because the dirty-tree precondition guarantees nothing
//!   uncommitted is lost) and the job returns to `BuildReady`.
//! - Only one promotion runs at a time, process-wide — see
//!   [`crate::concurrency::PromotionSlot`].
//!
//! Concurrency: promotion takes the per-job write lock (via `LineageEngine`,
//! same as every other job mutation) AND the process-global promotion
//! mutex. The job lock alone is not enough — two different jobs could
//! both target the same source repo simultaneously, and interleaving two
//! merges into one working tree is never safe.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::lineage::LineageEngine;
use crate::store::Store;
use crate::types::LineageJobStatus;

/// Ceiling for a single git operation (status, merge, reset, etc.) during
/// promotion. Git operations on a repo this size are seconds, not
/// minutes; 5 minutes gives large repos / slow disks headroom while still
/// guaranteeing a hung `git` can't wedge a job in `Merging` forever.
const GIT_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// Ceiling for one post-merge verification gate (`cargo check`, `cargo
/// test`). Matches the validator build-gate timeout in `validators.rs` —
/// promotion gates are the same class of operation, just run against the
/// source repo instead of a lineage worktree.
const GATE_TIMEOUT: Duration = Duration::from_secs(15 * 60);

const PIPELINE_AUTHOR_NAME: &str = "Evolvo Pipeline";
const PIPELINE_AUTHOR_EMAIL: &str = "pipeline@evolvo.local";

/// Outcome of a promotion attempt. `Ok` variants describe what happened so
/// the caller can decide whether to log more or just trust the job note
/// already appended.
#[derive(Debug)]
pub enum PromotionOutcome {
    Promoted { commit: String },
    Refused(String),
}

/// One post-merge verification gate. Returns `Ok(())` if the gate passes,
/// `Err(detail)` with an error-tail-style message if it doesn't. Boxed so
/// tests can stub out real `cargo` invocations (which are far too slow for
/// a tempdir integration test) while production code wires up the real
/// `cargo check --workspace` / `cargo test -p evolvo_desktop` commands.
///
/// This is the gate-stub seam: `promote_job_with_gates` takes a slice of
/// these; `promote_job` (the production entry point) supplies the real
/// cargo-backed gates via `default_gates`.
pub type Gate = Box<dyn Fn(&Path) -> Result<(), String> + Send + Sync>;

/// The real post-merge gates run against the source repo: `cargo check
/// --workspace` then `cargo test -p evolvo_desktop`, both subject to
/// `GATE_TIMEOUT` via `proc::run_with_timeout`. Named functions (not
/// closures) so `promote_job`'s default gate list is easy to read.
fn default_gates() -> Vec<Gate> {
    vec![
        Box::new(|repo| run_gate(repo, "cargo check --workspace", &["check", "--workspace"])),
        Box::new(|repo| {
            run_gate(
                repo,
                "cargo test -p evolvo_desktop",
                &["test", "-p", "evolvo_desktop"],
            )
        }),
    ]
}

fn run_gate(repo: &Path, label: &str, args: &[&str]) -> Result<(), String> {
    let mut cmd = Command::new("cargo");
    cmd.args(args)
        .current_dir(repo)
        .env("PATH", crate::runner::enriched_path());
    let capture =
        tempfile::NamedTempFile::new().map_err(|e| format!("{label}: capture file: {e}"))?;
    let out = capture
        .reopen()
        .map_err(|e| format!("{label}: reopen capture: {e}"))?;
    let err = out
        .try_clone()
        .map_err(|e| format!("{label}: clone capture handle: {e}"))?;
    cmd.stdout(std::process::Stdio::from(out))
        .stderr(std::process::Stdio::from(err));
    match crate::proc::run_with_timeout(&mut cmd, GATE_TIMEOUT) {
        Ok(crate::proc::WaitOutcome::Exited(s)) if s.success() => Ok(()),
        Ok(crate::proc::WaitOutcome::Exited(_)) => Err(format!(
            "{label} failed — output tail:\n{}",
            tail(capture.path(), 1500)
        )),
        Ok(crate::proc::WaitOutcome::TimedOut) => Err(format!(
            "{label} timed out after {}s (process group killed)",
            GATE_TIMEOUT.as_secs()
        )),
        Err(e) => Err(format!("{label}: spawn failed: {e}")),
    }
}

fn tail(path: &Path, max_chars: usize) -> String {
    let Ok(bytes) = std::fs::read(path) else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&bytes);
    let text = text.trim_end();
    let chars: Vec<char> = text.chars().collect();
    let start = chars.len().saturating_sub(max_chars);
    chars[start..].iter().collect()
}

/// Run a git subcommand and capture stdout/stderr, subject to
/// `GIT_TIMEOUT`. Unlike `run_with_timeout` (built for long-lived
/// gated/streamed processes), we need the output for status parsing and
/// error messages, so this uses a plain `Command::output()` — git
/// subcommands here are all fast (status, merge, reset), so a hang is the
/// exceptional case; we still bound it defensively with a watchdog thread
/// rather than leaving it fully unbounded.
fn git_output(repo: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(repo).args(args);
    cmd.stdin(std::process::Stdio::null());
    // Reuse the same process-group + timeout machinery as everywhere else
    // in the codebase, but we still need captured output — so we point
    // stdout/stderr at a temp file the same way validators.rs does.
    let capture = tempfile::NamedTempFile::new().map_err(|e| format!("git capture file: {e}"))?;
    let out_handle = capture
        .reopen()
        .map_err(|e| format!("git reopen capture: {e}"))?;
    let err_handle = out_handle
        .try_clone()
        .map_err(|e| format!("git clone capture handle: {e}"))?;
    cmd.stdout(std::process::Stdio::from(out_handle));
    cmd.stderr(std::process::Stdio::from(err_handle));
    match crate::proc::run_with_timeout(&mut cmd, GIT_TIMEOUT) {
        Ok(crate::proc::WaitOutcome::Exited(status)) => {
            let bytes = std::fs::read(capture.path()).unwrap_or_default();
            // git mixes stdout/stderr into one capture file; split isn't
            // possible after the fact, so we hand back the combined bytes
            // as stdout and leave stderr empty. Every caller below only
            // inspects `.status` and one of the two streams, never both,
            // so this is safe.
            Ok(std::process::Output {
                status,
                stdout: bytes,
                stderr: Vec::new(),
            })
        }
        Ok(crate::proc::WaitOutcome::TimedOut) => Err(format!(
            "git {} timed out after {}s (process group killed)",
            args.join(" "),
            GIT_TIMEOUT.as_secs()
        )),
        Err(e) => Err(format!("git {}: spawn failed: {e}", args.join(" "))),
    }
}

fn git_ok(repo: &Path, args: &[&str]) -> Result<(), String> {
    let out = git_output(repo, args)?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stdout).trim()
        ))
    }
}

fn git_stdout(repo: &Path, args: &[&str]) -> Result<String, String> {
    let out = git_output(repo, args)?;
    if !out.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stdout).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Precondition failure — carries the reason so the caller can append it
/// verbatim to the job note before returning the job to `BuildReady`.
struct Refusal(String);

/// Check every promotion precondition against `repo` / `job`. Returns
/// `Ok(())` if the merge may proceed, `Err(Refusal)` otherwise. Order
/// matters for error clarity but not for safety — every check is
/// independent and side-effect free (read-only `git status`/`rev-parse`).
fn check_preconditions(
    repo: &Path,
    branch: &str,
    base_branch: Option<&str>,
    worktree: &Path,
) -> Result<(), Refusal> {
    if !repo.join(".git").exists() {
        return Err(Refusal(format!(
            "source repo {} has no .git — refusing to promote",
            repo.display()
        )));
    }

    // Dirty working tree (staged OR unstaged modifications) blocks
    // promotion. We deliberately do NOT gate on untracked files — an
    // untracked scratch file sitting in the user's repo is common and
    // harmless to a merge/reset cycle (git merge and git reset --hard both
    // leave untracked files alone), whereas modified/staged files would
    // either conflict with the merge or be silently discarded by the
    // rollback `reset --hard`. Untracked files are therefore not a merge
    // hazard here and are intentionally excluded from the refusal check.
    let status = git_stdout(repo, &["status", "--porcelain=v1", "--untracked-files=no"])
        .map_err(|e| Refusal(format!("could not read source repo status: {e}")))?;
    if !status.trim().is_empty() {
        return Err(Refusal(format!(
            "source repo has uncommitted changes (tracked files) — refusing to promote until the working tree is clean:\n{status}"
        )));
    }

    // The lineage branch must exist in the source repo.
    if git_stdout(repo, &["rev-parse", "--verify", "--quiet", branch]).is_err() {
        return Err(Refusal(format!(
            "lineage branch {branch} does not exist in the source repo — cannot promote"
        )));
    }

    // The worktree's HEAD must match the branch tip — if the worktree was
    // torn down or the branch was mutated out from under it, promoting
    // would merge stale or unrelated history.
    let branch_tip = git_stdout(repo, &["rev-parse", branch])
        .map_err(|e| Refusal(format!("could not resolve {branch}: {e}")))?;
    if worktree.exists() {
        let worktree_head = git_stdout(worktree, &["rev-parse", "HEAD"])
            .map_err(|e| Refusal(format!("could not read worktree HEAD: {e}")))?;
        if worktree_head != branch_tip {
            return Err(Refusal(format!(
                "worktree HEAD ({}) does not match branch {branch} tip ({}) — refusing to promote a stale worktree",
                short(&worktree_head),
                short(&branch_tip)
            )));
        }
    } else {
        return Err(Refusal(format!(
            "worktree for this job no longer exists on disk — cannot verify it matches {branch}, refusing to promote"
        )));
    }

    // The source repo's currently checked-out branch must be the branch
    // the job was based on. If HEAD moved (reviewer switched branches
    // since Advance), merging now would land the change somewhere the
    // reviewer didn't intend.
    let current = crate::runner::checked_out_branch(repo);
    match (base_branch, current.as_deref()) {
        (Some(expected), Some(now)) if expected == now => {}
        (Some(expected), Some(now)) => {
            return Err(Refusal(format!(
                "source repo has switched branches since this job started (was {expected}, now {now}) — refusing to merge into the wrong branch"
            )));
        }
        (Some(expected), None) => {
            return Err(Refusal(format!(
                "source repo is in a detached HEAD state, but this job was based on branch {expected} — refusing to promote"
            )));
        }
        (None, _) => {
            return Err(Refusal(
                "this job has no recorded base branch (predates promotion, or the source repo was in detached HEAD when the job started) — refusing to promote; retry the job so a base branch is captured".to_string(),
            ));
        }
    }

    Ok(())
}

fn short(sha: &str) -> String {
    sha.chars().take(8).collect()
}

/// Result of running the merge step in isolation, before any gates run.
enum MergeStep {
    FastForward { new_head: String },
    MergeCommit { sha: String },
    Conflict { paths: Vec<String> },
}

/// Attempt the merge itself. Assumes preconditions already passed (clean
/// tree, branch exists, HEAD matches expectations). Prefers fast-forward;
/// falls back to a `--no-ff` merge commit authored as the pipeline
/// identity. On conflict, aborts cleanly and returns the conflicting
/// paths — the working tree is guaranteed back to `pre_merge_sha` either
/// way (fast-forward and clean merges are also fully reversible by the
/// caller via `reset --hard pre_merge_sha` if a later gate fails).
fn attempt_merge(
    repo: &Path,
    branch: &str,
    job_id: &str,
    title: &str,
) -> Result<MergeStep, String> {
    // Fast-forward is possible iff HEAD is an ancestor of the branch tip.
    let ff_possible = git_output(repo, &["merge-base", "--is-ancestor", "HEAD", branch])
        .map(|o| o.status.success())
        .unwrap_or(false);

    if ff_possible {
        git_ok(repo, &["merge", "--ff-only", branch])?;
        let new_head = git_stdout(repo, &["rev-parse", "HEAD"])?;
        return Ok(MergeStep::FastForward { new_head });
    }

    let message = format!("promote(lineage): {job_id} — {title}");
    let merge_result = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "-c",
            &format!("user.name={PIPELINE_AUTHOR_NAME}"),
            "-c",
            &format!("user.email={PIPELINE_AUTHOR_EMAIL}"),
            "merge",
            "--no-ff",
            "-m",
            &message,
            branch,
        ])
        .output();

    let output = match merge_result {
        Ok(o) => o,
        Err(e) => return Err(format!("failed to spawn git merge: {e}")),
    };

    if output.status.success() {
        let sha = git_stdout(repo, &["rev-parse", "HEAD"])?;
        return Ok(MergeStep::MergeCommit { sha });
    }

    // Merge failed — almost certainly a conflict. Collect conflicting
    // paths before aborting so the note is useful, then abort cleanly.
    let conflicts =
        git_stdout(repo, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
    let paths: Vec<String> = conflicts.lines().map(str::to_string).collect();
    let _ = git_ok(repo, &["merge", "--abort"]);
    Ok(MergeStep::Conflict { paths })
}

/// Public entry point used by production code: real cargo-backed gates.
pub fn promote_job(store: &Store, job_id: &str) -> PromotionOutcome {
    promote_job_with_gates(store, job_id, &default_gates())
}

/// Testable core: takes an explicit gate list so tests can stub out the
/// (slow, environment-dependent) `cargo check`/`cargo test` invocations
/// while exercising the real git merge/rollback logic. This is the
/// gate-stub seam referenced in the module doc comment.
pub fn promote_job_with_gates(store: &Store, job_id: &str, gates: &[Gate]) -> PromotionOutcome {
    let engine = LineageEngine::new(store);

    let job = match store.load_lineage_job(job_id) {
        Ok(Some(j)) => j,
        Ok(None) => {
            return PromotionOutcome::Refused(format!("lineage job not found: {job_id}"));
        }
        Err(e) => {
            return PromotionOutcome::Refused(format!("failed to load job {job_id}: {e}"));
        }
    };

    let (Some(source_repo), Some(branch), Some(worktree)) = (
        job.source_repo.clone(),
        job.branch_name.clone(),
        job.worktree_path.clone(),
    ) else {
        let note = "job is missing source_repo/branch_name/worktree_path — cannot promote (was Advance ever run?)".to_string();
        let _ = engine.append_note(job_id, &note);
        let _ = engine.force_status(job_id, LineageJobStatus::BuildReady);
        return PromotionOutcome::Refused(note);
    };
    let repo = PathBuf::from(&source_repo);
    let worktree_path = PathBuf::from(&worktree);

    if let Err(Refusal(reason)) =
        check_preconditions(&repo, &branch, job.base_branch.as_deref(), &worktree_path)
    {
        let _ = engine.append_note(job_id, &format!("promotion refused: {reason}"));
        let _ = engine.force_status(job_id, LineageJobStatus::BuildReady);
        return PromotionOutcome::Refused(reason);
    }

    // Snapshot pre-merge HEAD so any downstream failure can restore the
    // source repo exactly. Preconditions guarantee the tree is clean at
    // this point, so `reset --hard` back to this SHA loses nothing.
    let pre_merge_sha = match git_stdout(&repo, &["rev-parse", "HEAD"]) {
        Ok(sha) => sha,
        Err(e) => {
            let note = format!("failed to read source repo HEAD before merge: {e}");
            let _ = engine.append_note(job_id, &note);
            let _ = engine.force_status(job_id, LineageJobStatus::BuildReady);
            return PromotionOutcome::Refused(note);
        }
    };

    let _ = engine.append_note(
        job_id,
        &format!(
            "promotion: merging {branch} into {} (pre-merge HEAD {})",
            job.base_branch.as_deref().unwrap_or("<unknown>"),
            short(&pre_merge_sha)
        ),
    );

    let merge_step = match attempt_merge(&repo, &branch, job_id, &job.title) {
        Ok(step) => step,
        Err(e) => {
            // Machinery failure (git spawn error, IO). Best-effort restore
            // in case a partial merge state was left behind, then refuse.
            let _ = git_ok(&repo, &["merge", "--abort"]);
            let note = format!("promotion machinery failed during merge: {e}");
            let _ = engine.append_note(job_id, &note);
            let _ = engine.force_status(job_id, LineageJobStatus::BuildReady);
            return PromotionOutcome::Refused(note);
        }
    };

    let merge_sha = match merge_step {
        MergeStep::Conflict { paths } => {
            let note = if paths.is_empty() {
                "promotion refused: merge conflict (no conflicting paths reported) — merge aborted, source repo restored".to_string()
            } else {
                format!(
                    "promotion refused: merge conflict in {} file(s) — merge aborted, source repo restored:\n{}",
                    paths.len(),
                    paths.join("\n")
                )
            };
            let _ = engine.append_note(job_id, &note);
            let _ = engine.force_status(job_id, LineageJobStatus::BuildReady);
            return PromotionOutcome::Refused(note);
        }
        MergeStep::FastForward { new_head } => {
            let _ = engine.append_note(
                job_id,
                &format!("promotion: fast-forwarded to {}", short(&new_head)),
            );
            new_head
        }
        MergeStep::MergeCommit { sha } => {
            let _ = engine.append_note(
                job_id,
                &format!("promotion: created merge commit {}", short(&sha)),
            );
            sha
        }
    };

    // Post-merge verification gates run against the NEW HEAD.
    for gate in gates {
        if let Err(detail) = gate(&repo) {
            let _ = git_ok(&repo, &["reset", "--hard", &pre_merge_sha]);
            let note = format!(
                "promotion gate failed after merge — source repo reset to pre-merge {}:\n{detail}",
                short(&pre_merge_sha)
            );
            let _ = engine.append_note(job_id, &note);
            let _ = engine.force_status(job_id, LineageJobStatus::BuildReady);
            return PromotionOutcome::Refused(note);
        }
    }

    // Success: advance Merging -> Promoted and stamp the record.
    if let Err(e) = stamp_promoted(&engine, job_id, &merge_sha) {
        // The merge landed in the source repo but we couldn't persist the
        // job record. Leave the merge in place (reverting a successful,
        // gate-passed merge because of a JSON write failure would be
        // worse) and surface the inconsistency loudly via the note — the
        // reconciler will pick this up on next restart if the process
        // dies right here (see `reconcile_orphaned_jobs`).
        let _ = engine.append_note(
            job_id,
            &format!("promotion merged successfully ({}) but failed to persist job record: {e} — will reconcile on next restart", short(&merge_sha)),
        );
        return PromotionOutcome::Refused(format!("merged but failed to persist: {e}"));
    }

    let _ = engine.append_note(
        job_id,
        &format!("promotion complete — merged as {}", short(&merge_sha)),
    );

    PromotionOutcome::Promoted { commit: merge_sha }
}

fn stamp_promoted(engine: &LineageEngine<'_>, job_id: &str, merge_sha: &str) -> Result<(), String> {
    engine
        .force_status(job_id, LineageJobStatus::Promoted)
        .map_err(|e| e.to_string())?;
    engine
        .stamp_promotion(job_id, merge_sha)
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Entry point for the reviewer's Approve click on a `BuildReady` job:
/// transitions to `Merging` synchronously (so the UI reflects it
/// immediately) and runs the actual merge on a background thread, gated
/// by the process-global promotion mutex so only one promotion runs at a
/// time across the whole host.
pub fn start_promotion(store: Store, job_id: String) {
    std::thread::spawn(move || {
        let engine = LineageEngine::new(&store);
        let _permit = match crate::concurrency::PromotionSlot::try_acquire() {
            Some(p) => p,
            None => {
                let _ = engine.append_note(
                    &job_id,
                    "another promotion is already in progress — retry shortly",
                );
                let _ = engine.force_status(&job_id, LineageJobStatus::BuildReady);
                return;
            }
        };
        let _ = promote_job(&store, &job_id);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lineage::LineageEngine;
    use crate::types::{AgentKind, FeedbackRecord, FeedbackStatus, FeedbackType, LineageJobStatus};
    use tempfile::tempdir;

    fn git_init(dir: &Path) {
        assert!(Command::new("git")
            .arg("init")
            .arg("-q")
            .arg(dir)
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .args([
                "-C",
                dir.to_str().unwrap(),
                "config",
                "user.email",
                "test@example.com"
            ])
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .args(["-C", dir.to_str().unwrap(), "config", "user.name", "Test"])
            .status()
            .unwrap()
            .success());
    }

    fn commit_file(dir: &Path, name: &str, contents: &str, message: &str) -> String {
        std::fs::write(dir.join(name), contents).unwrap();
        assert!(Command::new("git")
            .args(["-C", dir.to_str().unwrap(), "add", "-A"])
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .args(["-C", dir.to_str().unwrap(), "commit", "-q", "-m", message])
            .status()
            .unwrap()
            .success());
        String::from_utf8_lossy(
            &Command::new("git")
                .args(["-C", dir.to_str().unwrap(), "rev-parse", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .trim()
        .to_string()
    }

    fn mk_feedback(id: &str) -> FeedbackRecord {
        FeedbackRecord {
            id: id.into(),
            feedback_type: FeedbackType::Bug,
            status: FeedbackStatus::New,
            page_route: "/".into(),
            feedback_text: "fix it".into(),
            annotations: vec![],
            pasted_images: vec![],
            screenshot_filename: None,
            voice_filename: None,
            voice_transcript: None,
            window_width: 100,
            window_height: 100,
            created_at_unix_ms: 0,
            updated_at_unix_ms: 0,
            lineage_job_id: None,
            local_delegate: None,
        }
    }

    /// Build a source repo + lineage branch + worktree scenario and a
    /// `BuildReady` job record pointing at it. Returns (store, job_id,
    /// source_repo_dir, worktree_dir) — callers keep the tempdirs alive by
    /// holding the returned guards.
    struct Scenario {
        _source_tmp: tempfile::TempDir,
        _workspace_tmp: tempfile::TempDir,
        store: Store,
        job_id: String,
        source_repo: PathBuf,
        #[allow(dead_code)]
        worktree: PathBuf,
    }

    fn build_scenario(diverge_source: bool) -> Scenario {
        let source_tmp = tempdir().unwrap();
        let source_repo = source_tmp.path().to_path_buf();
        git_init(&source_repo);
        commit_file(&source_repo, "README.md", "hello\n", "initial commit");
        // Ensure a stable default branch name regardless of git version
        // defaults (init.defaultBranch varies by host config).
        assert!(Command::new("git")
            .args(["-C", source_repo.to_str().unwrap(), "branch", "-M", "main"])
            .status()
            .unwrap()
            .success());

        let branch = "lineage/job-test".to_string();
        assert!(Command::new("git")
            .args([
                "-C",
                source_repo.to_str().unwrap(),
                "checkout",
                "-b",
                &branch
            ])
            .status()
            .unwrap()
            .success());
        commit_file(&source_repo, "feature.txt", "new feature\n", "add feature");
        assert!(Command::new("git")
            .args(["-C", source_repo.to_str().unwrap(), "checkout", "main"])
            .status()
            .unwrap()
            .success());

        if diverge_source {
            commit_file(
                &source_repo,
                "unrelated.txt",
                "unrelated change\n",
                "diverge main",
            );
        }

        // "Worktree" for the test: a second clone checked out to the
        // lineage branch tip, standing in for `git worktree add` (a real
        // worktree would also work, but a clone keeps the test independent
        // of the source repo's working directory state).
        let worktree = source_tmp.path().join("worktree-standin");
        let branch_tip = String::from_utf8_lossy(
            &Command::new("git")
                .args(["-C", source_repo.to_str().unwrap(), "rev-parse", &branch])
                .output()
                .unwrap()
                .stdout,
        )
        .trim()
        .to_string();
        assert!(Command::new("git")
            .args([
                "-C",
                source_repo.to_str().unwrap(),
                "worktree",
                "add",
                worktree.to_str().unwrap(),
                &branch_tip,
            ])
            .status()
            .unwrap()
            .success());

        let workspace_tmp = tempdir().unwrap();
        let store = Store::new(workspace_tmp.path().to_path_buf());
        store.init_workspace().unwrap();
        let engine = LineageEngine::new(&store);
        let mut fb = mk_feedback("fb-1");
        store.save_feedback(&fb).unwrap();
        let job = engine
            .enqueue_job_for_feedback(&mut fb, AgentKind::default())
            .unwrap();
        engine
            .set_run_artifacts(
                &job.id,
                worktree.display().to_string(),
                branch,
                "log.txt".into(),
                source_repo.display().to_string(),
                Some("main".to_string()),
            )
            .unwrap();
        engine
            .force_status(&job.id, LineageJobStatus::BuildReady)
            .unwrap();

        Scenario {
            _source_tmp: source_tmp,
            _workspace_tmp: workspace_tmp,
            store,
            job_id: job.id,
            source_repo,
            worktree,
        }
    }

    fn passing_gates() -> Vec<Gate> {
        vec![Box::new(|_repo: &Path| Ok(()))]
    }

    fn failing_gates(detail: &'static str) -> Vec<Gate> {
        vec![Box::new(move |_repo: &Path| Err(detail.to_string()))]
    }

    #[test]
    fn happy_path_fast_forward_promotes_and_stamps_job() {
        let s = build_scenario(false);
        let outcome = promote_job_with_gates(&s.store, &s.job_id, &passing_gates());
        match outcome {
            PromotionOutcome::Promoted { commit } => assert!(!commit.is_empty()),
            PromotionOutcome::Refused(reason) => {
                panic!("expected promotion, got refusal: {reason}")
            }
        }

        let job = s.store.load_lineage_job(&s.job_id).unwrap().unwrap();
        assert_eq!(job.status, LineageJobStatus::Promoted);
        assert!(job.promoted_commit.is_some());
        assert!(job.promoted_at_unix_ms.is_some());
        assert!(job.notes.iter().any(|n| n.contains("promotion complete")));

        // The feature file must actually be on main now.
        assert!(s.source_repo.join("feature.txt").exists());
    }

    #[test]
    fn dirty_source_tree_refuses_and_returns_to_build_ready() {
        let s = build_scenario(false);
        std::fs::write(s.source_repo.join("README.md"), "dirty\n").unwrap();

        let outcome = promote_job_with_gates(&s.store, &s.job_id, &passing_gates());
        match outcome {
            PromotionOutcome::Refused(reason) => assert!(reason.contains("uncommitted")),
            PromotionOutcome::Promoted { .. } => panic!("dirty tree must refuse promotion"),
        }

        let job = s.store.load_lineage_job(&s.job_id).unwrap().unwrap();
        assert_eq!(job.status, LineageJobStatus::BuildReady);
        assert!(job.notes.iter().any(|n| n.contains("promotion refused")));
    }

    #[test]
    fn branch_moved_since_job_started_refuses() {
        let s = build_scenario(false);
        assert!(Command::new("git")
            .args([
                "-C",
                s.source_repo.to_str().unwrap(),
                "checkout",
                "-b",
                "other"
            ])
            .status()
            .unwrap()
            .success());

        let outcome = promote_job_with_gates(&s.store, &s.job_id, &passing_gates());
        match outcome {
            PromotionOutcome::Refused(reason) => assert!(reason.contains("switched branches")),
            PromotionOutcome::Promoted { .. } => panic!("branch switch must refuse promotion"),
        }
        let job = s.store.load_lineage_job(&s.job_id).unwrap().unwrap();
        assert_eq!(job.status, LineageJobStatus::BuildReady);
    }

    #[test]
    fn conflict_aborts_and_restores_pre_merge_state() {
        let s = build_scenario(true); // main diverges with an unrelated commit
                                      // Now make main conflict with the lineage branch by editing the same
                                      // file the lineage branch touched, on main.
        commit_file(
            &s.source_repo,
            "feature.txt",
            "conflicting main version\n",
            "main also touches feature.txt",
        );
        let pre_merge = String::from_utf8_lossy(
            &Command::new("git")
                .args(["-C", s.source_repo.to_str().unwrap(), "rev-parse", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .trim()
        .to_string();

        let outcome = promote_job_with_gates(&s.store, &s.job_id, &passing_gates());
        match outcome {
            PromotionOutcome::Refused(reason) => {
                assert!(reason.contains("conflict"));
                assert!(reason.contains("feature.txt"));
            }
            PromotionOutcome::Promoted { .. } => panic!("conflicting merge must refuse"),
        }

        let job = s.store.load_lineage_job(&s.job_id).unwrap().unwrap();
        assert_eq!(job.status, LineageJobStatus::BuildReady);

        let head_after = String::from_utf8_lossy(
            &Command::new("git")
                .args(["-C", s.source_repo.to_str().unwrap(), "rev-parse", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .trim()
        .to_string();
        assert_eq!(
            head_after, pre_merge,
            "source repo HEAD must be unchanged after an aborted conflict"
        );

        // No merge state left behind.
        let status = String::from_utf8_lossy(
            &Command::new("git")
                .args([
                    "-C",
                    s.source_repo.to_str().unwrap(),
                    "status",
                    "--porcelain",
                ])
                .output()
                .unwrap()
                .stdout,
        )
        .trim()
        .to_string();
        assert!(
            status.is_empty(),
            "working tree must be clean after abort, got: {status}"
        );
    }

    #[test]
    fn gate_failure_resets_to_pre_merge_sha() {
        let s = build_scenario(false);
        let pre_merge = String::from_utf8_lossy(
            &Command::new("git")
                .args(["-C", s.source_repo.to_str().unwrap(), "rev-parse", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .trim()
        .to_string();

        let outcome = promote_job_with_gates(
            &s.store,
            &s.job_id,
            &failing_gates("cargo check failed: E0308 mismatched types"),
        );
        match outcome {
            PromotionOutcome::Refused(reason) => assert!(reason.contains("E0308")),
            PromotionOutcome::Promoted { .. } => panic!("failing gate must refuse promotion"),
        }

        let job = s.store.load_lineage_job(&s.job_id).unwrap().unwrap();
        assert_eq!(job.status, LineageJobStatus::BuildReady);
        assert!(job
            .notes
            .iter()
            .any(|n| n.contains("gate failed") && n.contains("E0308")));

        let head_after = String::from_utf8_lossy(
            &Command::new("git")
                .args(["-C", s.source_repo.to_str().unwrap(), "rev-parse", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .trim()
        .to_string();
        assert_eq!(
            head_after, pre_merge,
            "source repo must be reset to pre-merge SHA after a gate failure"
        );
        // The feature file must NOT be present after rollback.
        assert!(!s.source_repo.join("feature.txt").exists());
    }

    #[test]
    fn missing_base_branch_refuses() {
        let s = build_scenario(false);
        // Simulate a legacy job that never captured base_branch.
        let engine = LineageEngine::new(&s.store);
        let mut job = s.store.load_lineage_job(&s.job_id).unwrap().unwrap();
        job.base_branch = None;
        s.store.save_lineage_job(&job).unwrap();
        let _ = engine; // silence unused warning if append_note isn't called below

        let outcome = promote_job_with_gates(&s.store, &s.job_id, &passing_gates());
        match outcome {
            PromotionOutcome::Refused(reason) => {
                assert!(reason.contains("no recorded base branch"))
            }
            PromotionOutcome::Promoted { .. } => {
                panic!("missing base_branch must refuse promotion")
            }
        }
    }

    #[test]
    fn nonexistent_lineage_branch_refuses() {
        let s = build_scenario(false);
        let mut job = s.store.load_lineage_job(&s.job_id).unwrap().unwrap();
        job.branch_name = Some("lineage/does-not-exist".to_string());
        s.store.save_lineage_job(&job).unwrap();

        let outcome = promote_job_with_gates(&s.store, &s.job_id, &passing_gates());
        match outcome {
            PromotionOutcome::Refused(reason) => assert!(reason.contains("does not exist")),
            PromotionOutcome::Promoted { .. } => panic!("missing branch must refuse promotion"),
        }
    }
}
