//! Multi-stage NewApp pipeline orchestration.
//!
//! For a `FeedbackType::NewApp` feedback with a canvas attached, the runner
//! dispatches a sequence of Claude sessions — one per `StageKind` — each
//! reading and mutating a shared `plan.json` in the job's workspace.
//! Between sessions, a deterministic validator (see `validators.rs`) decides
//! whether the stage is `Green` or `Failed`. No LLM reviewer sits in the
//! middle.
//!
//! This module owns the *control flow* — seeding the plan, advancing stages,
//! and writing `StageState` updates back to the `LineageJobRecord`. The
//! actual Claude dispatch lives in `runner.rs` (it already knows how to
//! spawn `claude -p ...`); this module calls back into a small
//! `StageDispatcher` trait so the runner can provide the concrete spawner
//! and tests can provide a stub.

use std::path::{Path, PathBuf};

use crate::lineage::LineageEngine;
use crate::plan::{
    append_history, extract_regions, load_plan, save_plan, AppIdentity, CanvasReference,
    HistoryKind, IterationPlan, PlanStage, PLAN_SCHEMA_VERSION,
};
use crate::store::{Store, StoreError};
use crate::types::{FeedbackRecord, StageKind, StageState, StageStatus};
use crate::validators::{
    validate_backend_impl, validate_backend_plan, validate_e2e_impl, validate_e2e_plan,
    validate_final, validate_frontend_impl, validate_frontend_plan, StageReport,
};

/// Context handed to a dispatcher when a stage needs to run. Carries all the
/// on-disk pointers the Claude session needs: the plan file, the canvas PNG,
/// the worktree root. Dispatcher returns the path to the stage's log file.
pub struct StageDispatch<'a> {
    pub stage: StageKind,
    pub worktree: &'a Path,
    pub job_dir: &'a Path,
    pub plan_path: PathBuf,
    pub canvas_png: Option<PathBuf>,
    pub job_id: &'a str,
}

/// How the runner executes a stage. Production impl spawns `claude -p ...`
/// with the stage-specific prompt; tests use a no-op that just writes a
/// minimal plan section so the validator can be exercised.
pub trait StageDispatcher {
    /// Run the stage. On success, return the absolute path to the stage's
    /// captured log (so the UI can tail it). On failure, return a
    /// human-readable error string.
    fn dispatch(&self, ctx: StageDispatch<'_>) -> Result<PathBuf, String>;
}

/// Seed `plan.json` from a freshly-enqueued feedback record. Safe to call
/// multiple times — if a plan already exists on disk it is returned
/// untouched.
pub fn seed_plan_from_feedback(
    store: &Store,
    feedback: &FeedbackRecord,
    job_id: &str,
    job_dir: &Path,
    iteration: u32,
) -> Result<IterationPlan, StoreError> {
    if let Some(existing) = load_plan(job_dir)? {
        return Ok(existing);
    }
    std::fs::create_dir_all(job_dir)?;
    let inputs_dir = job_dir.join("inputs");
    std::fs::create_dir_all(&inputs_dir)?;

    let canvas_png = stage_canvas_png(store, feedback, &inputs_dir)?;
    let annotations_path = stage_annotations(feedback, &inputs_dir)?;
    let regions = extract_regions(&feedback.annotations);

    let plan = IterationPlan {
        schema_version: PLAN_SCHEMA_VERSION,
        app: AppIdentity {
            name: derive_app_name(&feedback.feedback_text),
            one_liner: truncate(&feedback.feedback_text, 120),
            domain: String::new(),
            feedback_id: feedback.id.clone(),
            job_id: job_id.to_string(),
            iteration,
        },
        stage: PlanStage::Seeded,
        canvas: CanvasReference {
            png_path: canvas_png.map(|p| p.display().to_string()),
            annotations_path: annotations_path.map(|p| p.display().to_string()),
            route: feedback.page_route.clone(),
            viewport_width: feedback.window_width,
            viewport_height: feedback.window_height,
            stroke_count: feedback.annotations.len() as u32,
            regions,
        },
        user_text: feedback.feedback_text.clone(),
        voice_transcript: feedback.voice_transcript.clone(),
        templates: Default::default(),
        backend: Default::default(),
        frontend: Default::default(),
        e2e: Default::default(),
        history: Vec::new(),
    };
    save_plan(job_dir, &plan)?;
    Ok(plan)
}

/// Copy the canvas screenshot from the feedback attachments dir into the
/// job's `inputs/` dir so every stage works from a stable, worktree-local
/// path. Returns None if the feedback has no canvas screenshot.
fn stage_canvas_png(
    store: &Store,
    feedback: &FeedbackRecord,
    inputs_dir: &Path,
) -> Result<Option<PathBuf>, StoreError> {
    let Some(filename) = feedback.screenshot_filename.as_ref() else {
        return Ok(None);
    };
    let bytes = match store.read_attachment(&feedback.id, filename)? {
        Some(b) => b,
        None => return Ok(None),
    };
    let dest = inputs_dir.join("canvas.png");
    std::fs::write(&dest, bytes)?;
    Ok(Some(dest))
}

fn stage_annotations(
    feedback: &FeedbackRecord,
    inputs_dir: &Path,
) -> Result<Option<PathBuf>, StoreError> {
    if feedback.annotations.is_empty() {
        return Ok(None);
    }
    let dest = inputs_dir.join("annotations.json");
    let json = serde_json::to_string_pretty(&feedback.annotations)?;
    std::fs::write(&dest, json)?;
    Ok(Some(dest))
}

/// File name of the styles.css snapshot stashed under the job's `inputs/`
/// dir at pipeline start. `validate_frontend_impl` compares the current
/// `app/ui/styles.css` against this baseline to prove that the FrontendImpl
/// stage actually added CSS rules for the NewApp components (a regression
/// Claude has shipped more than once — a behaviourally-correct but
/// unstyled app).
pub const STYLES_BASELINE_FILENAME: &str = "styles.baseline.css";

/// Copy `<worktree>/app/ui/styles.css` into
/// `<job_dir>/inputs/styles.baseline.css` the first time the pipeline runs
/// in this worktree. Idempotent — if the snapshot already exists (resume
/// path) the existing baseline is kept so size comparisons stay stable
/// across retries.
pub fn snapshot_styles_baseline(worktree: &Path, job_dir: &Path) -> std::io::Result<()> {
    let src = worktree.join("app/ui/styles.css");
    if !src.exists() {
        return Ok(());
    }
    let inputs_dir = job_dir.join("inputs");
    std::fs::create_dir_all(&inputs_dir)?;
    let dst = inputs_dir.join(STYLES_BASELINE_FILENAME);
    if dst.exists() {
        return Ok(());
    }
    std::fs::copy(&src, &dst)?;
    Ok(())
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    s.chars().take(n).collect::<String>() + "…"
}

/// Very small heuristic: take the first sentence / line, cap at 48 chars.
/// Used only to seed `app.name`; the BackendPlan stage is expected to
/// rewrite this with a deliberate name.
fn derive_app_name(text: &str) -> String {
    let first = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("NewApp");
    let cut = first
        .split(|c: char| matches!(c, '.' | '!' | '?' | ':' | ','))
        .next()
        .unwrap_or(first)
        .trim();
    let capped: String = cut.chars().take(48).collect();
    if capped.is_empty() {
        "NewApp".to_string()
    } else {
        capped
    }
}

/// Ordinal used to decide whether a stage's work is already on disk (i.e. the
/// planner or implementer already wrote its section of the plan in a previous
/// run). `Failed` and `Seeded` always resolve to zero so resuming those
/// restarts from `BackendPlan`.
fn plan_stage_ordinal(s: PlanStage) -> u32 {
    match s {
        PlanStage::Seeded | PlanStage::Failed => 0,
        PlanStage::BackendPlanned => 1,
        PlanStage::BackendImplemented => 2,
        PlanStage::FrontendPlanned => 3,
        PlanStage::FrontendImplemented => 4,
        PlanStage::E2EPlanned => 5,
        PlanStage::E2EImplemented => 6,
        PlanStage::Completed => 7,
    }
}

fn stage_completion_ordinal(k: StageKind) -> u32 {
    match k {
        StageKind::BackendPlan => 1,
        StageKind::BackendImpl => 2,
        StageKind::FrontendPlan => 3,
        StageKind::FrontendImpl => 4,
        StageKind::E2EPlan => 5,
        StageKind::E2EImpl => 6,
        StageKind::FinalReview => 7,
    }
}

/// True iff `plan.stage` indicates this `stage`'s artefacts are already
/// persisted (so resuming can skip the Claude dispatch and go straight to
/// the validator).
fn stage_already_done(plan_stage: PlanStage, stage: StageKind) -> bool {
    plan_stage_ordinal(plan_stage) >= stage_completion_ordinal(stage)
}

/// Run one stage end-to-end: flip to Running, dispatch, reload the plan,
/// run the validator, and persist the resulting `StageReport` on the job.
/// Returns `Ok(true)` if the stage passed, `Ok(false)` if the validator
/// rejected it, `Err(_)` on I/O or dispatcher errors.
///
/// **Resume behaviour:** if `plan.stage` on disk says this stage's output is
/// already written (e.g. `BackendPlanned` for the `BackendPlan` stage), the
/// Claude dispatch is skipped and the validator runs against the existing
/// plan. This makes re-running the pipeline idempotent — useful after a
/// deserialisation hiccup or transient dispatcher failure.
pub fn run_stage<D: StageDispatcher>(
    store: &Store,
    engine: &LineageEngine<'_>,
    dispatcher: &D,
    stage: StageKind,
    worktree: &Path,
    job_dir: &Path,
    job_id: &str,
) -> Result<bool, String> {
    let existing_plan_stage = load_plan(job_dir)
        .ok()
        .flatten()
        .map(|p| p.stage)
        .unwrap_or(PlanStage::Seeded);
    let resume_skip = stage_already_done(existing_plan_stage, stage);

    if resume_skip {
        let _ = engine.append_note(
            job_id,
            &format!(
                "resume: plan.stage={} — skipping Claude dispatch for {} and re-validating existing artifacts",
                existing_plan_stage.label(),
                stage.slug()
            ),
        );
        engine
            .update_stage(job_id, stage, |s| {
                s.status = StageStatus::Validating;
                s.headline = Some(format!(
                    "resumed — re-validating existing {} output",
                    stage.slug()
                ));
            })
            .map_err(|e| format!("mark validating (resume): {e}"))?;
    } else {
        engine
            .update_stage(job_id, stage, |s| {
                s.status = StageStatus::Running;
                s.headline = Some(format!("running {}", stage.slug()));
            })
            .map_err(|e| format!("mark stage running: {e}"))?;

        let plan_path = job_dir.join(crate::plan::PLAN_FILENAME);
        let canvas_png = load_plan(job_dir)
            .ok()
            .flatten()
            .and_then(|p| p.canvas.png_path.map(PathBuf::from));

        let dispatch = StageDispatch {
            stage,
            worktree,
            job_dir,
            plan_path,
            canvas_png,
            job_id,
        };
        let log_path = dispatcher.dispatch(dispatch).map_err(|e| {
            let _ = engine.update_stage(job_id, stage, |s| {
                s.status = StageStatus::Failed;
                s.headline = Some(format!("dispatcher error: {e}"));
            });
            e
        })?;

        engine
            .update_stage(job_id, stage, |s| {
                s.status = StageStatus::Validating;
                s.log_path = Some(log_path.display().to_string());
            })
            .map_err(|e| format!("mark validating: {e}"))?;
    }

    let plan = load_plan(job_dir)
        .map_err(|e| format!("reload plan: {e}"))?
        .ok_or_else(|| "plan.json missing after stage ran".to_string())?;

    let report = run_validator(stage, &plan, worktree, job_dir);
    persist_stage_outcome(store, engine, job_id, job_dir, stage, &report)?;
    Ok(report.passed)
}

fn run_validator(
    stage: StageKind,
    plan: &IterationPlan,
    worktree: &Path,
    job_dir: &Path,
) -> StageReport {
    match stage {
        StageKind::BackendPlan => validate_backend_plan(plan),
        StageKind::FrontendPlan => validate_frontend_plan(plan),
        StageKind::E2EPlan => validate_e2e_plan(plan),
        StageKind::BackendImpl => validate_backend_impl(plan, worktree),
        StageKind::FrontendImpl => validate_frontend_impl(plan, worktree, job_dir),
        StageKind::E2EImpl => validate_e2e_impl(plan, worktree),
        StageKind::FinalReview => validate_final(plan),
    }
}

fn persist_stage_outcome(
    _store: &Store,
    engine: &LineageEngine<'_>,
    job_id: &str,
    job_dir: &Path,
    stage: StageKind,
    report: &StageReport,
) -> Result<(), String> {
    let report_json = report.to_json();
    let headline = report.headline.clone();
    let passed = report.passed;
    engine
        .update_stage(job_id, stage, |s| {
            s.status = if passed {
                StageStatus::Green
            } else {
                StageStatus::Failed
            };
            s.headline = Some(headline);
            s.report = Some(report_json);
        })
        .map_err(|e| format!("persist stage outcome: {e}"))?;

    if let Ok(Some(mut plan)) = load_plan(job_dir) {
        let kind = if passed {
            HistoryKind::Validated
        } else {
            HistoryKind::Rejected
        };
        let _ = append_history(job_dir, &mut plan, stage.slug(), kind, &report.headline);
        if passed {
            advance_plan_stage(&mut plan, stage);
            let _ = save_plan(job_dir, &plan);
        }
    }
    Ok(())
}

fn advance_plan_stage(plan: &mut IterationPlan, finished: StageKind) {
    plan.stage = match finished {
        StageKind::BackendPlan => PlanStage::BackendPlanned,
        StageKind::BackendImpl => PlanStage::BackendImplemented,
        StageKind::FrontendPlan => PlanStage::FrontendPlanned,
        StageKind::FrontendImpl => PlanStage::FrontendImplemented,
        StageKind::E2EPlan => PlanStage::E2EPlanned,
        StageKind::E2EImpl => PlanStage::E2EImplemented,
        StageKind::FinalReview => PlanStage::Completed,
    };
}

/// Run the full canonical pipeline, stopping at the first red stage.
/// Returns the list of stages actually executed.
pub fn run_pipeline<D: StageDispatcher>(
    store: &Store,
    engine: &LineageEngine<'_>,
    dispatcher: &D,
    worktree: &Path,
    job_dir: &Path,
    job_id: &str,
) -> Result<Vec<StageState>, String> {
    engine
        .seed_stages(job_id, StageKind::pipeline())
        .map_err(|e| format!("seed stages: {e}"))?;
    for stage in StageKind::pipeline().iter().copied() {
        let ok = run_stage(store, engine, dispatcher, stage, worktree, job_dir, job_id)?;
        if !ok {
            break;
        }
    }
    let job = store
        .load_lineage_job(job_id)
        .map_err(|e| format!("reload job: {e}"))?
        .ok_or_else(|| format!("job {job_id} vanished"))?;
    Ok(job.stages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FeedbackStatus, FeedbackType};
    use serde_json::json;
    use tempfile::tempdir;

    fn mk_feedback() -> FeedbackRecord {
        FeedbackRecord {
            id: "fb-1".into(),
            feedback_type: FeedbackType::NewApp,
            status: FeedbackStatus::New,
            page_route: "/".into(),
            feedback_text: "Build a budget tracker. First line details.".into(),
            annotations: vec![
                json!({"x": 10, "y": 10, "w": 30, "h": 30, "color": "#ff0000"}),
                json!({"x": 25, "y": 15, "w": 30, "h": 30, "color": "#ff0000"}),
            ],
            pasted_images: vec![],
            screenshot_filename: None,
            voice_filename: None,
            voice_transcript: None,
            window_width: 1440,
            window_height: 900,
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
            lineage_job_id: None,
        }
    }

    #[test]
    fn seed_plan_creates_plan_json_with_regions() {
        let tmp = tempdir().unwrap();
        let store = Store::new(tmp.path().to_path_buf());
        store.init_workspace().unwrap();
        let feedback = mk_feedback();
        let job_dir = tmp.path().join("job-1");
        let plan =
            seed_plan_from_feedback(&store, &feedback, "job-1", &job_dir, 1).unwrap();
        assert_eq!(plan.schema_version, PLAN_SCHEMA_VERSION);
        assert_eq!(plan.app.feedback_id, "fb-1");
        assert_eq!(plan.canvas.regions.len(), 1);
        assert!(plan.app.name.starts_with("Build a budget tracker"));
        assert!(job_dir.join("plan.json").exists());
    }

    #[test]
    fn seed_plan_is_idempotent() {
        let tmp = tempdir().unwrap();
        let store = Store::new(tmp.path().to_path_buf());
        store.init_workspace().unwrap();
        let feedback = mk_feedback();
        let job_dir = tmp.path().join("job-1");
        let a = seed_plan_from_feedback(&store, &feedback, "job-1", &job_dir, 1).unwrap();
        let b = seed_plan_from_feedback(&store, &feedback, "job-1", &job_dir, 1).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn derive_app_name_caps_at_48_and_strips_punct() {
        assert_eq!(derive_app_name(""), "NewApp");
        assert_eq!(derive_app_name("Budget Tracker. With transactions."), "Budget Tracker");
        let long = "x".repeat(100);
        assert_eq!(derive_app_name(&long).chars().count(), 48);
    }

    #[test]
    fn stage_already_done_respects_plan_stage_ordinal() {
        use PlanStage::*;
        use StageKind::*;
        // Fresh plan: nothing is done.
        for s in [BackendPlan, BackendImpl, FrontendPlan, FinalReview] {
            assert!(!stage_already_done(Seeded, s));
            assert!(!stage_already_done(Failed, s));
        }
        // Backend planned: only BackendPlan is done.
        assert!(stage_already_done(BackendPlanned, BackendPlan));
        assert!(!stage_already_done(BackendPlanned, BackendImpl));
        assert!(!stage_already_done(BackendPlanned, FrontendPlan));
        // Frontend implemented: everything through FrontendImpl is done.
        assert!(stage_already_done(FrontendImplemented, BackendPlan));
        assert!(stage_already_done(FrontendImplemented, BackendImpl));
        assert!(stage_already_done(FrontendImplemented, FrontendPlan));
        assert!(stage_already_done(FrontendImplemented, FrontendImpl));
        assert!(!stage_already_done(FrontendImplemented, E2EPlan));
        assert!(!stage_already_done(FrontendImplemented, FinalReview));
        // Completed: every stage is done.
        for s in [BackendPlan, BackendImpl, FrontendPlan, FrontendImpl, E2EPlan, E2EImpl, FinalReview] {
            assert!(stage_already_done(Completed, s), "stage {s:?} should be done when plan Completed");
        }
    }
}
