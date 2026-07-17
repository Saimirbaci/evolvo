//! Lightweight lineage engine.
//!
//! The lineage concept from agent_swarm is a gated pipeline that turns user
//! feedback into reviewable proposals. Here we keep just the state-machine and
//! persistence surface: every submitted feedback spawns a `LineageJobRecord`
//! that a reviewer can advance, approve, or reject. Rich automation (LLM
//! triage, build, merge) lives behind this boundary and can be added later
//! without changing the API.
//!
//! Safety invariants:
//! - Transitions go through `Transition::apply` which validates source state
//!   and timestamps every update.
//! - The store persists snapshots so a crash between transitions leaves a
//!   reviewable record, never partial binary state.

use crate::store::{Store, StoreError};
use crate::types::{
    current_time_unix_ms, AgentKind, FeedbackRecord, FeedbackStatus, FreedomLevel,
    LineageJobRecord, LineageJobStatus, StageKind, StageState, StageStatus,
};

pub struct LineageEngine<'a> {
    store: &'a Store,
}

/// Per-job write lock serialising every load-mutate-save on a
/// `LineageJobRecord`. The runner thread and the Tauri command thread both
/// mutate the same JSON file through this engine; without the lock, two
/// interleaved read-modify-write cycles silently drop one side's update
/// (e.g. a note appended by the runner vanishes when a status write from
/// the main thread lands a millisecond later — the race documented at
/// `commands::resume_lineage_job`). Process-wide because engines are
/// constructed ad-hoc per call; atomic file writes alone cannot fix this,
/// only serialise the torn file, so the lock is load-bearing.
fn job_write_lock(job_id: &str) -> std::sync::Arc<std::sync::Mutex<()>> {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, OnceLock};
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();
    let map = LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = match map.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.entry(job_id.to_string()).or_default().clone()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    Triage,
    Plan,
    StartImplementation,
    MarkBuildReady,
    Approve,
    Reject,
    Fail,
}

impl Transition {
    pub fn is_valid_from(self, from: LineageJobStatus) -> bool {
        use LineageJobStatus as S;
        use Transition as T;
        matches!(
            (self, from),
            (T::Triage, S::Pending)
                | (T::Plan, S::Triaging)
                | (T::StartImplementation, S::Planned)
                | (T::MarkBuildReady, S::Implementing)
                // NOTE: `BuildReady` is deliberately excluded here. Promoting
                // out of `BuildReady` must go through the real merge in
                // `promotion::start_promotion` (BuildReady -> Merging ->
                // Promoted/BuildReady), never this direct status flip — see
                // `commands::approve_lineage_job`, which intercepts
                // `BuildReady` before it ever reaches `LineageEngine::transition`.
                | (T::Approve, S::Pending | S::Planned)
                | (T::Reject, _)
                | (T::Fail, _)
        )
    }

    pub fn target(self, _from: LineageJobStatus) -> LineageJobStatus {
        match self {
            Self::Triage => LineageJobStatus::Triaging,
            Self::Plan => LineageJobStatus::Planned,
            Self::StartImplementation => LineageJobStatus::Implementing,
            Self::MarkBuildReady => LineageJobStatus::BuildReady,
            Self::Approve => LineageJobStatus::Promoted,
            Self::Reject => LineageJobStatus::Rejected,
            Self::Fail => LineageJobStatus::Failed,
        }
    }
}

impl<'a> LineageEngine<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    /// Create a fresh pending lineage job for a piece of feedback and link
    /// the two records atomically (from the caller's perspective). The
    /// `agent` parameter records which CLI backend should run the job so
    /// Retry / Resume stay consistent with the user's original selection.
    pub fn enqueue_job_for_feedback(
        &self,
        feedback: &mut FeedbackRecord,
        agent: AgentKind,
    ) -> Result<LineageJobRecord, StoreError> {
        let now = current_time_unix_ms();
        let id = format!("job-{now}");
        let job = LineageJobRecord {
            id: id.clone(),
            feedback_id: feedback.id.clone(),
            title: derive_title(feedback),
            summary: feedback.feedback_text.clone(),
            status: LineageJobStatus::Pending,
            notes: Vec::new(),
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
            worktree_path: None,
            branch_name: None,
            log_path: None,
            source_repo: None,
            iteration: 0,
            freedom: FreedomLevel::Auto,
            stages: Vec::new(),
            agent,
            pause_after_plan: false,
            plan_approved_at_unix_ms: None,
            base_branch: None,
            promoted_at_unix_ms: None,
            promoted_commit: None,
        };
        self.store.save_lineage_job(&job)?;

        feedback.lineage_job_id = Some(id);
        feedback.status = FeedbackStatus::InLineage;
        feedback.updated_at_unix_ms = now;
        self.store.save_feedback(feedback)?;
        Ok(job)
    }

    pub fn transition(
        &self,
        job_id: &str,
        transition: Transition,
    ) -> Result<LineageJobRecord, StoreError> {
        let lock = job_write_lock(job_id);
        let _guard = lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut job = self
            .store
            .load_lineage_job(job_id)?
            .ok_or_else(|| format!("lineage job not found: {job_id}"))?;
        if !transition.is_valid_from(job.status) {
            return Err(
                format!("transition {transition:?} not valid from {:?}", job.status).into(),
            );
        }
        job.status = transition.target(job.status);
        job.updated_at_unix_ms = current_time_unix_ms();
        self.store.save_lineage_job(&job)?;
        Ok(job)
    }

    pub fn append_note(&self, job_id: &str, note: &str) -> Result<LineageJobRecord, StoreError> {
        let lock = job_write_lock(job_id);
        let _guard = lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut job = self
            .store
            .load_lineage_job(job_id)?
            .ok_or_else(|| format!("lineage job not found: {job_id}"))?;
        if !note.trim().is_empty() {
            job.notes.push(note.trim().to_string());
            job.updated_at_unix_ms = current_time_unix_ms();
            self.store.save_lineage_job(&job)?;
        }
        Ok(job)
    }

    /// Unconditional status write for the async runner. The `transition`
    /// path enforces the human-review state machine; this one is reserved
    /// for machine-driven transitions (claude started / finished / failed)
    /// and must only be called from trusted paths.
    pub fn force_status(
        &self,
        job_id: &str,
        status: LineageJobStatus,
    ) -> Result<LineageJobRecord, StoreError> {
        let lock = job_write_lock(job_id);
        let _guard = lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut job = self
            .store
            .load_lineage_job(job_id)?
            .ok_or_else(|| format!("lineage job not found: {job_id}"))?;
        job.status = status;
        job.updated_at_unix_ms = current_time_unix_ms();
        self.store.save_lineage_job(&job)?;
        Ok(job)
    }

    /// Seed the `stages` vec on a job with the canonical pipeline order —
    /// one `StageState::pending` per `StageKind`. Idempotent: re-seeding an
    /// already-populated vec is a no-op so restarts / retries don't drop
    /// progress. Returns the refreshed record.
    pub fn seed_stages(
        &self,
        job_id: &str,
        stages: &[StageKind],
    ) -> Result<LineageJobRecord, StoreError> {
        let lock = job_write_lock(job_id);
        let _guard = lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut job = self
            .store
            .load_lineage_job(job_id)?
            .ok_or_else(|| format!("lineage job not found: {job_id}"))?;
        if !job.stages.is_empty() {
            return Ok(job);
        }
        job.stages = stages.iter().copied().map(StageState::pending).collect();
        job.updated_at_unix_ms = current_time_unix_ms();
        self.store.save_lineage_job(&job)?;
        Ok(job)
    }

    /// Mutate a single stage entry by kind. Closure receives `&mut
    /// StageState` and is expected to update status / timestamps / headline.
    /// Auto-stamps `started_at_unix_ms` the first time the stage flips to
    /// `Running` and `finished_at_unix_ms` when it reaches a terminal status.
    pub fn update_stage<F>(
        &self,
        job_id: &str,
        kind: StageKind,
        mutator: F,
    ) -> Result<LineageJobRecord, StoreError>
    where
        F: FnOnce(&mut StageState),
    {
        let lock = job_write_lock(job_id);
        let _guard = lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut job = self
            .store
            .load_lineage_job(job_id)?
            .ok_or_else(|| format!("lineage job not found: {job_id}"))?;
        let slot = job
            .stages
            .iter_mut()
            .find(|s| s.kind == kind)
            .ok_or_else(|| format!("stage {kind:?} not seeded for job {job_id}"))?;
        let prev_status = slot.status;
        mutator(slot);
        let now = current_time_unix_ms();
        if matches!(slot.status, StageStatus::Running | StageStatus::Validating)
            && slot.started_at_unix_ms.is_none()
        {
            slot.started_at_unix_ms = Some(now);
        }
        if slot.status.is_terminal() && slot.finished_at_unix_ms.is_none() {
            slot.finished_at_unix_ms = Some(now);
        }
        if prev_status != slot.status {
            job.updated_at_unix_ms = now;
        }
        self.store.save_lineage_job(&job)?;
        Ok(job)
    }

    /// Arm the plan-review gate on a job: the pipeline will stop after the
    /// BackendPlan stage validates and wait for reviewer approval.
    pub fn request_plan_review(&self, job_id: &str) -> Result<LineageJobRecord, StoreError> {
        let lock = job_write_lock(job_id);
        let _guard = lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut job = self
            .store
            .load_lineage_job(job_id)?
            .ok_or_else(|| format!("lineage job not found: {job_id}"))?;
        job.pause_after_plan = true;
        job.updated_at_unix_ms = current_time_unix_ms();
        self.store.save_lineage_job(&job)?;
        Ok(job)
    }

    /// Record the reviewer's approval of the backend plan. Once set, the
    /// plan-review pause never fires again for this job, so resuming the
    /// pipeline proceeds straight into implementation.
    pub fn mark_plan_approved(&self, job_id: &str) -> Result<LineageJobRecord, StoreError> {
        let lock = job_write_lock(job_id);
        let _guard = lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut job = self
            .store
            .load_lineage_job(job_id)?
            .ok_or_else(|| format!("lineage job not found: {job_id}"))?;
        let now = current_time_unix_ms();
        job.plan_approved_at_unix_ms = Some(now);
        job.updated_at_unix_ms = now;
        self.store.save_lineage_job(&job)?;
        Ok(job)
    }

    /// Record the worktree / branch / log / source paths produced by the
    /// runner when a job first enters `Implementing`. `base_branch` is the
    /// branch the source repo had checked out at worktree-creation time
    /// (`None` if it could not be resolved, e.g. detached HEAD) — promotion
    /// later refuses to merge if the source repo has since switched to a
    /// different branch, so this snapshot is load-bearing, not cosmetic.
    pub fn set_run_artifacts(
        &self,
        job_id: &str,
        worktree_path: String,
        branch_name: String,
        log_path: String,
        source_repo: String,
        base_branch: Option<String>,
    ) -> Result<LineageJobRecord, StoreError> {
        let lock = job_write_lock(job_id);
        let _guard = lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut job = self
            .store
            .load_lineage_job(job_id)?
            .ok_or_else(|| format!("lineage job not found: {job_id}"))?;
        job.worktree_path = Some(worktree_path);
        job.branch_name = Some(branch_name);
        job.log_path = Some(log_path);
        job.source_repo = Some(source_repo);
        job.base_branch = base_branch;
        job.updated_at_unix_ms = current_time_unix_ms();
        self.store.save_lineage_job(&job)?;
        Ok(job)
    }

    /// Stamp a successful promotion merge onto the job record:
    /// `promotedAtUnixMs` + `promotedCommit`. Does NOT change `status` —
    /// callers pair this with `force_status(.., Promoted)` (see
    /// `promotion::stamp_promoted`), kept separate so each write stays a
    /// single clear responsibility under the job lock.
    pub fn stamp_promotion(
        &self,
        job_id: &str,
        merge_sha: &str,
    ) -> Result<LineageJobRecord, StoreError> {
        let lock = job_write_lock(job_id);
        let _guard = lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut job = self
            .store
            .load_lineage_job(job_id)?
            .ok_or_else(|| format!("lineage job not found: {job_id}"))?;
        job.promoted_at_unix_ms = Some(current_time_unix_ms());
        job.promoted_commit = Some(merge_sha.to_string());
        job.updated_at_unix_ms = current_time_unix_ms();
        self.store.save_lineage_job(&job)?;
        Ok(job)
    }
}

fn derive_title(feedback: &FeedbackRecord) -> String {
    let first_line = feedback.feedback_text.lines().next().unwrap_or("").trim();
    if first_line.is_empty() {
        format!("Feedback {}", feedback.id)
    } else if first_line.len() > 80 {
        format!("{}…", &first_line[..80])
    } else {
        first_line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::FeedbackType;
    use tempfile::tempdir;

    fn mk_feedback(id: &str) -> FeedbackRecord {
        FeedbackRecord {
            id: id.into(),
            feedback_type: FeedbackType::Bug,
            status: FeedbackStatus::New,
            page_route: "/".into(),
            feedback_text: "button should be rounder".into(),
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

    #[test]
    fn concurrent_note_appends_do_not_lose_updates() {
        // The documented lost-update race: runner thread and main thread
        // both load-mutate-save the same job record. With the per-job
        // write lock every append survives; without it this test flakes
        // hard (interleaved read-modify-write drops notes).
        let temp = tempdir().unwrap();
        let store = Store::new(temp.path().to_path_buf());
        store.init_workspace().unwrap();
        let engine = LineageEngine::new(&store);
        let mut fb = mk_feedback("fb-race");
        store.save_feedback(&fb).unwrap();
        let job = engine
            .enqueue_job_for_feedback(&mut fb, AgentKind::default())
            .unwrap();

        let store_t = store.clone();
        let id_t = job.id.clone();
        let t = std::thread::spawn(move || {
            let engine = LineageEngine::new(&store_t);
            for i in 0..50 {
                engine
                    .append_note(&id_t, &format!("thread note {i}"))
                    .unwrap();
            }
        });
        for i in 0..50 {
            engine
                .append_note(&job.id, &format!("main note {i}"))
                .unwrap();
        }
        t.join().unwrap();

        let back = store.load_lineage_job(&job.id).unwrap().unwrap();
        assert_eq!(
            back.notes.len(),
            100,
            "every append from both threads must survive"
        );
    }

    #[test]
    fn seed_and_update_stages_tracks_status_and_timestamps() {
        let temp = tempdir().unwrap();
        let store = Store::new(temp.path().to_path_buf());
        store.init_workspace().unwrap();
        let engine = LineageEngine::new(&store);
        let mut fb = mk_feedback("fb-1");
        let job = engine
            .enqueue_job_for_feedback(&mut fb, AgentKind::default())
            .unwrap();

        let pipeline = [
            crate::types::StageKind::BackendPlan,
            crate::types::StageKind::BackendImpl,
        ];
        let seeded = engine.seed_stages(&job.id, &pipeline).unwrap();
        assert_eq!(seeded.stages.len(), 2);
        assert!(seeded
            .stages
            .iter()
            .all(|s| matches!(s.status, crate::types::StageStatus::Pending)));

        let updated = engine
            .update_stage(&job.id, crate::types::StageKind::BackendPlan, |s| {
                s.status = crate::types::StageStatus::Running;
                s.headline = Some("dispatching planner".into());
            })
            .unwrap();
        let plan = updated
            .stages
            .iter()
            .find(|s| matches!(s.kind, crate::types::StageKind::BackendPlan))
            .unwrap();
        assert!(matches!(plan.status, crate::types::StageStatus::Running));
        assert!(plan.started_at_unix_ms.is_some());
        assert!(plan.finished_at_unix_ms.is_none());

        let done = engine
            .update_stage(&job.id, crate::types::StageKind::BackendPlan, |s| {
                s.status = crate::types::StageStatus::Green;
                s.headline = Some("plan.json written".into());
            })
            .unwrap();
        let plan = done
            .stages
            .iter()
            .find(|s| matches!(s.kind, crate::types::StageKind::BackendPlan))
            .unwrap();
        assert!(matches!(plan.status, crate::types::StageStatus::Green));
        assert!(plan.finished_at_unix_ms.is_some());

        // Re-seeding is idempotent.
        let again = engine.seed_stages(&job.id, &pipeline).unwrap();
        assert_eq!(again.stages.len(), 2);
        assert!(matches!(
            again.stages[0].status,
            crate::types::StageStatus::Green
        ));
    }

    #[test]
    fn enqueue_links_feedback_and_job() {
        let temp = tempdir().unwrap();
        let store = Store::new(temp.path().to_path_buf());
        store.init_workspace().unwrap();

        let engine = LineageEngine::new(&store);
        let mut fb = mk_feedback("fb-1");
        let job = engine
            .enqueue_job_for_feedback(&mut fb, AgentKind::default())
            .unwrap();

        assert_eq!(fb.lineage_job_id.as_deref(), Some(job.id.as_str()));
        assert_eq!(fb.status, FeedbackStatus::InLineage);
        assert_eq!(job.feedback_id, "fb-1");
        assert_eq!(job.status, LineageJobStatus::Pending);
        assert_eq!(job.title, "button should be rounder");
    }

    #[test]
    fn transition_enforces_state_machine() {
        let temp = tempdir().unwrap();
        let store = Store::new(temp.path().to_path_buf());
        store.init_workspace().unwrap();

        let engine = LineageEngine::new(&store);
        let mut fb = mk_feedback("fb-1");
        let job = engine
            .enqueue_job_for_feedback(&mut fb, AgentKind::default())
            .unwrap();

        // Pending -> Triaging ok
        let j2 = engine.transition(&job.id, Transition::Triage).unwrap();
        assert_eq!(j2.status, LineageJobStatus::Triaging);

        // Cannot go Pending -> BuildReady (invalid from Triaging state)
        let bad = engine.transition(&job.id, Transition::MarkBuildReady);
        assert!(bad.is_err());

        // Triaging -> Planned -> Implementing -> BuildReady
        engine.transition(&job.id, Transition::Plan).unwrap();
        engine
            .transition(&job.id, Transition::StartImplementation)
            .unwrap();
        let ready = engine
            .transition(&job.id, Transition::MarkBuildReady)
            .unwrap();
        assert_eq!(ready.status, LineageJobStatus::BuildReady);

        // BuildReady -> Approve is deliberately NOT a valid direct
        // transition through the engine: promotion out of BuildReady must
        // go through the real merge in `promotion::start_promotion`
        // (BuildReady -> Merging -> Promoted/BuildReady), which
        // `commands::approve_lineage_job` intercepts before it ever calls
        // `transition`. See the NOTE on `Transition::is_valid_from`.
        let rejected_direct_promote = engine.transition(&job.id, Transition::Approve);
        assert!(
            rejected_direct_promote.is_err(),
            "BuildReady -> Promoted must not be reachable as a direct engine transition"
        );
    }

    #[test]
    fn reject_and_fail_are_always_valid() {
        let temp = tempdir().unwrap();
        let store = Store::new(temp.path().to_path_buf());
        store.init_workspace().unwrap();

        let engine = LineageEngine::new(&store);
        let mut fb = mk_feedback("fb-1");
        let job = engine
            .enqueue_job_for_feedback(&mut fb, AgentKind::default())
            .unwrap();
        let out = engine.transition(&job.id, Transition::Reject).unwrap();
        assert_eq!(out.status, LineageJobStatus::Rejected);
    }

    #[test]
    fn append_note_grows_history() {
        let temp = tempdir().unwrap();
        let store = Store::new(temp.path().to_path_buf());
        store.init_workspace().unwrap();

        let engine = LineageEngine::new(&store);
        let mut fb = mk_feedback("fb-1");
        let job = engine
            .enqueue_job_for_feedback(&mut fb, AgentKind::default())
            .unwrap();
        let j2 = engine.append_note(&job.id, "needs more detail").unwrap();
        assert_eq!(j2.notes, vec!["needs more detail"]);
        let j3 = engine.append_note(&job.id, "   ").unwrap();
        assert_eq!(j3.notes.len(), 1);
    }
}
