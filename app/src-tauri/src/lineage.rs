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
    current_time_unix_ms, FeedbackRecord, FeedbackStatus, LineageJobRecord, LineageJobStatus,
    StageKind, StageState, StageStatus,
};

pub struct LineageEngine<'a> {
    store: &'a Store,
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
                | (T::Approve, S::Pending | S::Planned | S::BuildReady)
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
    /// the two records atomically (from the caller's perspective).
    pub fn enqueue_job_for_feedback(
        &self,
        feedback: &mut FeedbackRecord,
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
            stages: Vec::new(),
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

    /// Record the worktree / branch / log / source paths produced by the
    /// runner when a job first enters `Implementing`.
    pub fn set_run_artifacts(
        &self,
        job_id: &str,
        worktree_path: String,
        branch_name: String,
        log_path: String,
        source_repo: String,
    ) -> Result<LineageJobRecord, StoreError> {
        let mut job = self
            .store
            .load_lineage_job(job_id)?
            .ok_or_else(|| format!("lineage job not found: {job_id}"))?;
        job.worktree_path = Some(worktree_path);
        job.branch_name = Some(branch_name);
        job.log_path = Some(log_path);
        job.source_repo = Some(source_repo);
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
        }
    }

    #[test]
    fn seed_and_update_stages_tracks_status_and_timestamps() {
        let temp = tempdir().unwrap();
        let store = Store::new(temp.path().to_path_buf());
        store.init_workspace().unwrap();
        let engine = LineageEngine::new(&store);
        let mut fb = mk_feedback("fb-1");
        let job = engine.enqueue_job_for_feedback(&mut fb).unwrap();

        let pipeline = [
            crate::types::StageKind::BackendPlan,
            crate::types::StageKind::BackendImpl,
        ];
        let seeded = engine.seed_stages(&job.id, &pipeline).unwrap();
        assert_eq!(seeded.stages.len(), 2);
        assert!(seeded.stages.iter().all(|s| matches!(s.status, crate::types::StageStatus::Pending)));

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
        let job = engine.enqueue_job_for_feedback(&mut fb).unwrap();

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
        let job = engine.enqueue_job_for_feedback(&mut fb).unwrap();

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

        // BuildReady -> Approve -> Promoted
        let promoted = engine.transition(&job.id, Transition::Approve).unwrap();
        assert_eq!(promoted.status, LineageJobStatus::Promoted);
    }

    #[test]
    fn reject_and_fail_are_always_valid() {
        let temp = tempdir().unwrap();
        let store = Store::new(temp.path().to_path_buf());
        store.init_workspace().unwrap();

        let engine = LineageEngine::new(&store);
        let mut fb = mk_feedback("fb-1");
        let job = engine.enqueue_job_for_feedback(&mut fb).unwrap();
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
        let job = engine.enqueue_job_for_feedback(&mut fb).unwrap();
        let j2 = engine.append_note(&job.id, "needs more detail").unwrap();
        assert_eq!(j2.notes, vec!["needs more detail"]);
        let j3 = engine.append_note(&job.id, "   ").unwrap();
        assert_eq!(j3.notes.len(), 1);
    }
}
