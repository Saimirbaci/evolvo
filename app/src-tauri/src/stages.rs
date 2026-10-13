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
    HistoryKind, IterationPlan, PlanStage, StoryboardScreenReference, PLAN_SCHEMA_VERSION,
};
use crate::store::{Store, StoreError};
use crate::types::{FeedbackRecord, StageKind, StageState, StageStatus};
use crate::validators::{
    validate_backend_impl, validate_backend_plan, validate_e2e_impl, validate_e2e_plan,
    validate_final, validate_frontend_impl, validate_frontend_plan, StageReport,
};

/// Maximum number of agent dispatches for a single stage: the first attempt
/// plus up to two Reflexion retries where the validator's failed checks are
/// rendered back into the prompt (see `previous_failure` on
/// [`StageDispatch`]). Deliberately small — every attempt is a full agent
/// session, and if two corrections don't converge the failure is structural
/// and a human should look at it.
pub const MAX_STAGE_ATTEMPTS: u32 = 3;

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
    /// 1-based attempt counter. 1 = first dispatch of this stage in this
    /// pipeline run; >1 = Reflexion retry after a validator rejection.
    pub attempt: u32,
    /// When `attempt > 1`, the previous attempt's failed validator checks
    /// (from [`StageReport::failure_block`]). The dispatcher renders this
    /// into the prompt so the agent fixes the specific findings instead of
    /// re-running blind.
    pub previous_failure: Option<String>,
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

    let canvas = if feedback.screens.is_empty() {
        let canvas_png = stage_canvas_png(store, feedback, &inputs_dir)?;
        let annotations_path = stage_annotations(feedback, &inputs_dir)?;
        let regions = extract_regions(
            &feedback.annotations,
            feedback.window_width,
            feedback.window_height,
        );
        CanvasReference {
            png_path: canvas_png.map(|p| p.display().to_string()),
            annotations_path: annotations_path.map(|p| p.display().to_string()),
            route: feedback.page_route.clone(),
            viewport_width: feedback.window_width,
            viewport_height: feedback.window_height,
            stroke_count: feedback.annotations.len() as u32,
            regions,
            screens: Vec::new(),
        }
    } else {
        stage_storyboard(store, feedback, &inputs_dir)?
    };

    // Seed the backend budget so the planner sees a region-scaled
    // floor from the start. The planner is free to raise these
    // values; the validator clamps to max(plan_value, floor) at
    // validation time, so a planner that "lazily" downgrades the
    // budget still gets held to the floor.
    let backend = crate::plan::BackendSection {
        budget: crate::plan::budget_floor_for_regions(canvas.regions.len()),
        ..Default::default()
    };

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
        canvas,
        user_text: feedback.feedback_text.clone(),
        voice_transcript: feedback.voice_transcript.clone(),
        templates: Default::default(),
        backend,
        frontend: Default::default(),
        e2e: Default::default(),
        history: Vec::new(),
        open_questions: Vec::new(),
        answered_questions: Vec::new(),
    };
    save_plan(job_dir, &plan)?;
    Ok(plan)
}

fn stage_storyboard(
    store: &Store,
    feedback: &FeedbackRecord,
    inputs_dir: &Path,
) -> Result<CanvasReference, StoreError> {
    let mut screens = Vec::with_capacity(feedback.screens.len());
    let mut all_regions = Vec::new();
    let mut total_strokes = 0u32;

    for (idx, screen) in feedback.screens.iter().enumerate() {
        let ordinal = idx + 1;
        let png_path = if let Some(filename) = screen.screenshot_filename.as_ref() {
            match store.read_attachment(&feedback.id, filename)? {
                Some(bytes) => {
                    let dest = inputs_dir.join(format!("screen-{ordinal:02}.png"));
                    std::fs::write(&dest, bytes)?;
                    Some(dest.display().to_string())
                }
                None => None,
            }
        } else {
            None
        };
        let annotations_path = if screen.annotations.is_empty() {
            None
        } else {
            let dest = inputs_dir.join(format!("screen-{ordinal:02}.annotations.json"));
            std::fs::write(&dest, serde_json::to_string_pretty(&screen.annotations)?)?;
            Some(dest.display().to_string())
        };
        let mut pasted_image_paths = Vec::with_capacity(screen.pasted_images.len());
        for filename in &screen.pasted_images {
            if let Some(bytes) = store.read_attachment(&feedback.id, filename)? {
                let dest = inputs_dir.join(filename);
                std::fs::write(&dest, bytes)?;
                pasted_image_paths.push(dest.display().to_string());
            }
        }
        let mut regions = extract_regions(
            &screen.annotations,
            screen.window_width,
            screen.window_height,
        );
        for region in &mut regions {
            region.id = format!("S{ordinal}-{}", region.id);
        }
        total_strokes = total_strokes.saturating_add(screen.annotations.len() as u32);
        all_regions.extend(regions.iter().cloned());
        screens.push(StoryboardScreenReference {
            id: screen.id.clone(),
            name: screen.name.clone(),
            description: screen.description.clone(),
            intended_route: screen.intended_route.clone(),
            png_path,
            annotations_path,
            pasted_image_paths,
            viewport_width: screen.window_width,
            viewport_height: screen.window_height,
            regions,
        });
    }

    let manifest = inputs_dir.join("storyboard.json");
    std::fs::write(&manifest, serde_json::to_string_pretty(&screens)?)?;
    let first = screens.first();
    Ok(CanvasReference {
        png_path: first.and_then(|screen| screen.png_path.clone()),
        annotations_path: first.and_then(|screen| screen.annotations_path.clone()),
        route: first
            .map(|screen| screen.intended_route.clone())
            .unwrap_or_else(|| feedback.page_route.clone()),
        viewport_width: first
            .map(|screen| screen.viewport_width)
            .unwrap_or_default(),
        viewport_height: first
            .map(|screen| screen.viewport_height)
            .unwrap_or_default(),
        stroke_count: total_strokes,
        regions: all_regions,
        screens,
    })
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
///
/// Legacy name, legacy meaning: this snapshot now targets whichever file is
/// the iteration-owned stylesheet in this worktree — `app/ui/styles/app.css`
/// if it exists (the current layered layout), else `app/ui/styles.css` (a
/// worktree cloned from before the tokens/base/app split).
pub const STYLES_BASELINE_FILENAME: &str = "styles.baseline.css";

/// Path to the iteration-owned stylesheet in `worktree`: the layered
/// `app/ui/styles/app.css` when present, else the legacy monolithic
/// `app/ui/styles.css`. Centralised here so the snapshot step and the
/// validator agree on which file is "the" NewApp stylesheet.
pub fn iteration_styles_path(worktree: &Path) -> PathBuf {
    let layered = worktree.join("app/ui/styles/app.css");
    if layered.exists() {
        layered
    } else {
        worktree.join("app/ui/styles.css")
    }
}

/// Copy the worktree's iteration-owned stylesheet (see
/// [`iteration_styles_path`]) into `<job_dir>/inputs/styles.baseline.css`
/// the first time the pipeline runs in this worktree. Idempotent — if the
/// snapshot already exists (resume path) the existing baseline is kept so
/// size comparisons stay stable across retries.
pub fn snapshot_styles_baseline(worktree: &Path, job_dir: &Path) -> std::io::Result<()> {
    let src = iteration_styles_path(worktree);
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
        .split(['.', '!', '?', ':', ','])
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
/// rejected every attempt, `Err(_)` on I/O or dispatcher errors.
///
/// **Reflexion retries:** a validator rejection does not end the stage —
/// up to [`MAX_STAGE_ATTEMPTS`] dispatches run, each retry carrying the
/// previous attempt's failed checks in `StageDispatch::previous_failure`
/// so the agent fixes the specific findings against the worktree state the
/// failed attempt left behind. A malformed or missing `plan.json` after a
/// dispatch counts as a failed attempt (with the parse error fed back),
/// not a hard pipeline error.
///
/// **Resume behaviour:** if `plan.stage` on disk says this stage's output is
/// already written (e.g. `BackendPlanned` for the `BackendPlan` stage), the
/// first attempt skips the Claude dispatch and validates the existing
/// artifacts; if that validation fails, the retry attempts dispatch
/// normally. This keeps re-running the pipeline idempotent — useful after a
/// deserialisation hiccup or transient dispatcher failure.
///
/// **Checkpoint:** when a stage validates green, the worktree is
/// checkpoint-committed (best-effort) so later recovery can reset to the
/// last green stage instead of discarding the whole worktree.
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
    let mut previous_failure: Option<String> = None;

    for attempt in 1..=MAX_STAGE_ATTEMPTS {
        // The agent's own STAGE_RESULT declaration for this attempt (only
        // available when a dispatch actually ran). Advisory — the validator
        // below stays authoritative — but it travels into notes and the
        // Reflexion feedback for legibility.
        let mut declared: Option<crate::runner::StageResultDecl> = None;
        if attempt == 1 && resume_skip {
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
            let headline = if attempt == 1 {
                format!("running {}", stage.slug())
            } else {
                format!(
                    "retrying {} (attempt {attempt}/{MAX_STAGE_ATTEMPTS}) with validator findings",
                    stage.slug()
                )
            };
            engine
                .update_stage(job_id, stage, |s| {
                    s.status = StageStatus::Running;
                    s.headline = Some(headline);
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
                attempt,
                previous_failure: previous_failure.clone(),
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

            declared = crate::runner::read_stage_result(&log_path);
            if let Some(decl) = declared.as_ref().filter(|d| !d.ok) {
                let _ = engine.append_note(
                    job_id,
                    &format!(
                        "{}: agent exited 0 but declared STAGE_RESULT: fail: {}",
                        stage.slug(),
                        decl.reason.as_deref().unwrap_or("<no reason given>")
                    ),
                );
            }
        }

        // A plan that no longer loads is a *failed attempt*, not a hard
        // error — the agent gets the parse failure fed back and a chance
        // to repair the file it broke.
        let report = match load_plan(job_dir) {
            Ok(Some(plan)) => run_validator(stage, &plan, worktree, job_dir),
            Ok(None) => crate::validators::report_failure(
                stage.slug(),
                "plan.json exists after stage ran",
                "plan.json missing after stage ran",
            ),
            Err(e) => crate::validators::report_failure(
                stage.slug(),
                "plan.json parses after stage ran",
                &format!("plan.json failed to parse: {e}"),
            ),
        };
        let report = persist_stage_outcome(store, engine, job_id, job_dir, stage, report)?;

        if report.passed {
            if let Some(sha) = checkpoint_worktree(worktree, stage, job_id) {
                let _ = engine.append_note(
                    job_id,
                    &format!(
                        "checkpoint: committed {} green state as {sha}",
                        stage.slug()
                    ),
                );
            }
            return Ok(true);
        }

        let mut feedback = report.failure_block();
        if let Some(decl) = declared.as_ref().filter(|d| !d.ok) {
            feedback = format!(
                "agent self-declared failure: {}\n{feedback}",
                decl.reason.as_deref().unwrap_or("<no reason given>")
            );
        }
        previous_failure = Some(feedback);
        if attempt < MAX_STAGE_ATTEMPTS {
            let _ = engine.append_note(
                job_id,
                &format!(
                    "{} rejected by validator (attempt {attempt}/{MAX_STAGE_ATTEMPTS}) — retrying with the failed checks in the prompt",
                    stage.slug()
                ),
            );
        }
    }

    let _ = engine.append_note(
        job_id,
        &format!(
            "{} still rejected after {MAX_STAGE_ATTEMPTS} attempts — giving up on this stage",
            stage.slug()
        ),
    );
    Ok(false)
}

/// Best-effort checkpoint commit after a stage validates green: stage
/// everything in the worktree and commit it as
/// `stage(<slug>): green checkpoint — job <id>` under a fixed pipeline
/// identity. Gives every green stage a durable git anchor so recovery can
/// `git reset --hard` to the last good state instead of discarding the
/// worktree. Returns the short sha, or `None` when the directory is not a
/// git repo, there is nothing to commit (plan-only stages), or git fails —
/// checkpointing is bookkeeping, never a gate, so it cannot fail the stage.
fn checkpoint_worktree(worktree: &Path, stage: StageKind, job_id: &str) -> Option<String> {
    let git = |args: &[&str]| -> Option<std::process::Output> {
        std::process::Command::new("git")
            .arg("-C")
            .arg(worktree)
            .args(args)
            .output()
            .ok()
    };

    let inside = git(&["rev-parse", "--is-inside-work-tree"])?;
    if !inside.status.success() {
        return None;
    }
    if !git(&["add", "-A"])?.status.success() {
        return None;
    }
    // `diff --cached --quiet` exits 0 when nothing is staged.
    if git(&["diff", "--cached", "--quiet"])?.status.success() {
        return None;
    }
    let message = format!("stage({}): green checkpoint — job {}", stage.slug(), job_id);
    let commit = git(&[
        "-c",
        "user.name=Evolvo Pipeline",
        "-c",
        "user.email=pipeline@evolvo.local",
        "commit",
        "-m",
        &message,
    ])?;
    if !commit.status.success() {
        return None;
    }
    let sha = git(&["rev-parse", "--short", "HEAD"])?;
    if !sha.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&sha.stdout).trim().to_string())
}

/// Dispatch to the deterministic validator for `stage`. Public so the
/// `evolvo_desktop validate` CLI can expose the exact gate to an
/// in-worktree agent session (self-validation before exit).
pub fn run_validator(
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
        StageKind::FinalReview => validate_final(plan, worktree),
    }
}

/// Persist a stage's outcome onto the job record and (when the stage
/// passed) advance `plan.stage` on disk. Returns the report that was
/// actually recorded — which may be a **demoted** version of the input
/// when `report.passed` was true but the plan could not be reloaded and
/// re-persisted with the advanced stage marker.
///
/// This demotion matters: a stage must never be reported `Green` while
/// `plan.stage` silently fails to advance. Before this fix, a corrupted
/// `plan.json` after an otherwise-passing validator run would leave the
/// job record saying `Green` (the caller only looked at the *input*
/// `report.passed`) while `plan.stage` stayed frozen — the next stage's
/// `stage_already_done` / resume logic would then desync from the job
/// record's idea of progress. Now a load/save failure on the green path
/// is treated as a failed attempt, with the parse/write error rendered
/// into the failure detail so Reflexion retries see it.
fn persist_stage_outcome(
    _store: &Store,
    engine: &LineageEngine<'_>,
    job_id: &str,
    job_dir: &Path,
    stage: StageKind,
    report: StageReport,
) -> Result<StageReport, String> {
    let report = if report.passed {
        match load_plan(job_dir) {
            Ok(Some(mut plan)) => {
                let _ = append_history(
                    job_dir,
                    &mut plan,
                    stage.slug(),
                    HistoryKind::Validated,
                    &report.headline,
                );
                advance_plan_stage(&mut plan, stage);
                match save_plan(job_dir, &plan) {
                    Ok(_) => report,
                    Err(e) => crate::validators::report_failure(
                        stage.slug(),
                        "plan.json re-persisted after stage advanced",
                        &format!("stage validated but failed to save plan.json: {e}"),
                    ),
                }
            }
            Ok(None) => crate::validators::report_failure(
                stage.slug(),
                "plan.json exists after stage validated",
                "stage validated but plan.json is missing — cannot advance plan.stage",
            ),
            Err(e) => crate::validators::report_failure(
                stage.slug(),
                "plan.json parses after stage validated",
                &format!("stage validated but plan.json failed to reload: {e}"),
            ),
        }
    } else {
        // Failure path: best-effort history entry, never promoted to a
        // hard error — the pipeline already treats this attempt as
        // failed regardless of whether the history append lands.
        if let Ok(Some(mut plan)) = load_plan(job_dir) {
            let _ = append_history(
                job_dir,
                &mut plan,
                stage.slug(),
                HistoryKind::Rejected,
                &report.headline,
            );
        }
        report
    };

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

    Ok(report)
}

/// Advance `plan.stage` monotonically. During resume the pipeline re-runs
/// validators for already-green upstream stages; without the ordinal guard
/// below, finishing `BackendPlan` after a previous run reached
/// `FrontendImplemented` would regress `plan.stage` from ord 4 back to ord
/// 1, which makes `stage_already_done` return false for every subsequent
/// stage and causes the agent to be re-dispatched against artifacts that
/// already exist on disk.
fn advance_plan_stage(plan: &mut IterationPlan, finished: StageKind) {
    let candidate = match finished {
        StageKind::BackendPlan => PlanStage::BackendPlanned,
        StageKind::BackendImpl => PlanStage::BackendImplemented,
        StageKind::FrontendPlan => PlanStage::FrontendPlanned,
        StageKind::FrontendImpl => PlanStage::FrontendImplemented,
        StageKind::E2EPlan => PlanStage::E2EPlanned,
        StageKind::E2EImpl => PlanStage::E2EImplemented,
        StageKind::FinalReview => PlanStage::Completed,
    };
    if plan_stage_ordinal(candidate) > plan_stage_ordinal(plan.stage) {
        plan.stage = candidate;
    }
}

/// Run the full canonical pipeline, stopping at the first red stage.
/// Returns the list of stages actually executed.
/// Outcome of a full pipeline pass.
pub struct PipelineRun {
    pub stages: Vec<StageState>,
    /// True when the pipeline intentionally stopped after BackendPlan
    /// because the job requested a plan review — the job has been flipped
    /// to `Planned` and waits for the reviewer to approve (resume) or
    /// reject. NOT a failure; the caller must leave the status alone.
    pub paused_for_plan_review: bool,
}

pub fn run_pipeline<D: StageDispatcher>(
    store: &Store,
    engine: &LineageEngine<'_>,
    dispatcher: &D,
    worktree: &Path,
    job_dir: &Path,
    job_id: &str,
) -> Result<PipelineRun, String> {
    engine
        .seed_stages(job_id, StageKind::pipeline())
        .map_err(|e| format!("seed stages: {e}"))?;
    let mut paused_for_plan_review = false;
    for stage in StageKind::pipeline().iter().copied() {
        let ok = run_stage(store, engine, dispatcher, stage, worktree, job_dir, job_id)?;
        if !ok {
            break;
        }
        if stage == StageKind::BackendPlan {
            // A plan that opts out of live behaviour (static app) or guesses
            // the app can be simulated is a product decision: stop for the
            // reviewer even if they didn't ask for plan review.
            if let Some(reason) = forced_plan_review_reason(store, job_dir, job_id) {
                if engine.request_plan_review(job_id).is_ok() {
                    let _ = engine.append_note(
                        job_id,
                        &format!("plan review required — {reason}. Approve to build it as planned, or Reject and resubmit with clearer intent."),
                    );
                }
            }
        }
        if stage == StageKind::BackendPlan && plan_settled_by_answers(store, job_dir, job_id) {
            // "Answer & re-plan": every question the reviewer was asked now
            // has an answer and nothing else needs a decision — continue.
            if engine.mark_plan_approved(job_id).is_ok() {
                let _ = engine.append_note(
                    job_id,
                    "re-plan validated — every open question is answered; continuing into implementation",
                );
            }
        }
        if stage == StageKind::BackendPlan && should_pause_for_plan_review(store, job_id) {
            let _ = engine.append_note(
                job_id,
                "backend plan validated — pausing for plan review; check plan.json (incl. openQuestions), then Approve to start implementation or Reject",
            );
            let _ = engine.force_status(job_id, crate::types::LineageJobStatus::Planned);
            paused_for_plan_review = true;
            break;
        }
    }
    let job = store
        .load_lineage_job(job_id)
        .map_err(|e| format!("reload job: {e}"))?
        .ok_or_else(|| format!("job {job_id} vanished"))?;
    Ok(PipelineRun {
        stages: job.stages,
        paused_for_plan_review,
    })
}

/// Why the validated plan must be reviewed before implementation even when
/// the job didn't request it — `None` once the reviewer approved the plan
/// (so a resume never re-pauses) or when nothing needs a decision.
fn forced_plan_review_reason(store: &Store, job_dir: &Path, job_id: &str) -> Option<String> {
    let approved = store
        .load_lineage_job(job_id)
        .ok()
        .flatten()
        .is_some_and(|j| j.plan_approved_at_unix_ms.is_some());
    if approved {
        return None;
    }
    let plan = load_plan(job_dir).ok().flatten()?;
    crate::plan::plan_review_required_reason(&plan)
}

/// True when a paused-for-review job's re-plan has settled every question
/// (see `plan::plan_review_settled_by_answers`). Only applies before
/// approval; an approved job never re-enters the gate anyway.
fn plan_settled_by_answers(store: &Store, job_dir: &Path, job_id: &str) -> bool {
    let awaiting = store
        .load_lineage_job(job_id)
        .ok()
        .flatten()
        .is_some_and(|j| j.pause_after_plan && j.plan_approved_at_unix_ms.is_none());
    awaiting
        && load_plan(job_dir)
            .ok()
            .flatten()
            .is_some_and(|plan| crate::plan::plan_review_settled_by_answers(&plan))
}

/// The plan-review gate fires only while the job both requested it and has
/// not been approved yet — so a resume after approval sails straight
/// through BackendPlan's re-validation into implementation.
fn should_pause_for_plan_review(store: &Store, job_id: &str) -> bool {
    store
        .load_lineage_job(job_id)
        .ok()
        .flatten()
        .map(|j| j.pause_after_plan && j.plan_approved_at_unix_ms.is_none())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FeedbackScreenRecord, FeedbackStatus, FeedbackType};
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
            screens: vec![],
            voice_filename: None,
            voice_transcript: None,
            window_width: 1440,
            window_height: 900,
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
            lineage_job_id: None,
            revision: Default::default(),
            target_app: None,
            background_filename: None,
            auto_launch: false,
        }
    }

    #[test]
    fn seed_plan_creates_plan_json_with_regions() {
        let tmp = tempdir().unwrap();
        let store = Store::new(tmp.path().to_path_buf());
        store.init_workspace().unwrap();
        let feedback = mk_feedback();
        let job_dir = tmp.path().join("job-1");
        let plan = seed_plan_from_feedback(&store, &feedback, "job-1", &job_dir, 1).unwrap();
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
    fn seed_plan_stages_every_storyboard_screen_with_qualified_regions() {
        let tmp = tempdir().unwrap();
        let store = Store::new(tmp.path().to_path_buf());
        store.init_workspace().unwrap();
        let mut feedback = mk_feedback();
        store
            .save_attachment(&feedback.id, "screen-01.png", b"one")
            .unwrap();
        store
            .save_attachment(&feedback.id, "screen-02.png", b"two")
            .unwrap();
        feedback.screens = vec![
            FeedbackScreenRecord {
                id: "dashboard".into(),
                name: "Dashboard".into(),
                intended_route: "/".into(),
                annotations: vec![json!({"x": 0.1, "y": 0.1, "w": 0.2, "h": 0.2})],
                screenshot_filename: Some("screen-01.png".into()),
                window_width: 1000,
                window_height: 700,
                ..Default::default()
            },
            FeedbackScreenRecord {
                id: "settings".into(),
                name: "Settings".into(),
                description: "Edit account preferences".into(),
                intended_route: "/settings".into(),
                annotations: vec![json!({"x": 0.6, "y": 0.6, "w": 0.2, "h": 0.2})],
                screenshot_filename: Some("screen-02.png".into()),
                window_width: 1000,
                window_height: 700,
                ..Default::default()
            },
        ];

        let job_dir = tmp.path().join("job-storyboard");
        let plan =
            seed_plan_from_feedback(&store, &feedback, "job-storyboard", &job_dir, 1).unwrap();

        assert_eq!(plan.canvas.screens.len(), 2);
        assert_eq!(plan.canvas.regions.len(), 2);
        assert_eq!(plan.canvas.regions[0].id, "S1-R1");
        assert_eq!(plan.canvas.regions[1].id, "S2-R1");
        assert!(job_dir.join("inputs/screen-01.png").exists());
        assert!(job_dir.join("inputs/screen-02.png").exists());
        assert!(job_dir.join("inputs/storyboard.json").exists());
    }

    #[test]
    fn derive_app_name_caps_at_48_and_strips_punct() {
        assert_eq!(derive_app_name(""), "NewApp");
        assert_eq!(
            derive_app_name("Budget Tracker. With transactions."),
            "Budget Tracker"
        );
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
        for s in [
            BackendPlan,
            BackendImpl,
            FrontendPlan,
            FrontendImpl,
            E2EPlan,
            E2EImpl,
            FinalReview,
        ] {
            assert!(
                stage_already_done(Completed, s),
                "stage {s:?} should be done when plan Completed"
            );
        }
    }

    /// Scripted stand-in for the agent: records every dispatch (attempt +
    /// previous_failure) and, from `fix_on_attempt` onward, writes a plan
    /// that satisfies `validate_backend_plan`. Before that it leaves the
    /// seeded (incomplete) plan in place so the validator rejects.
    struct ScriptedDispatcher {
        job_dir: PathBuf,
        fix_on_attempt: u32,
        calls: std::cell::RefCell<Vec<(u32, Option<String>)>>,
    }

    impl StageDispatcher for ScriptedDispatcher {
        fn dispatch(&self, ctx: StageDispatch<'_>) -> Result<PathBuf, String> {
            self.calls
                .borrow_mut()
                .push((ctx.attempt, ctx.previous_failure.clone()));
            if ctx.attempt >= self.fix_on_attempt {
                let mut plan = load_plan(&self.job_dir).unwrap().unwrap();
                plan.canvas.png_path = Some("/tmp/canvas.png".into());
                plan.backend.entities.push(crate::plan::EntityPlan {
                    name: "Task".into(),
                    fields: vec![],
                    motivated_by_regions: vec!["R1".into()],
                });
                for verb in ["create_task", "list_tasks", "update_task", "delete_task"] {
                    plan.backend.commands.push(crate::plan::CommandPlan {
                        name: verb.into(),
                        input: "Payload".into(),
                        output: "Task".into(),
                        motivated_by_regions: vec!["R1".into()],
                        summary: "does the thing".into(),
                        body_sketch:
                            "1. Decode payload. 2. Persist via store.save_task. 3. Return record."
                                .into(),
                        anchors: vec!["save_task".into()],
                    });
                }
                plan.backend.behaviors.push(crate::plan::BehaviorPlan {
                    name: "overdue_tasks_flagged".into(),
                    trigger: "timer".into(),
                    triggered_by: "every 60s".into(),
                    effect: "tasks past their due date show as overdue".into(),
                    changes_state: vec!["Task.overdue".into()],
                    engine: "deterministic".into(),
                    implemented_in: "flag_overdue_tasks".into(),
                    body_sketch:
                        "List tasks, set overdue when due < now, persist via store.save_task."
                            .into(),
                    anchors: vec!["save_task".into()],
                    motivated_by_regions: vec!["R1".into()],
                });
                for t in ["create_persists", "list_returns_all", "delete_removes"] {
                    plan.backend.tests.push(crate::plan::TestPlan {
                        name: t.into(),
                        module: "store::tests".into(),
                        covers: "crud".into(),
                    });
                }
                save_plan(&self.job_dir, &plan).unwrap();
            }
            let log = self.job_dir.join("logs").join("backend_plan.log");
            std::fs::create_dir_all(log.parent().unwrap()).unwrap();
            std::fs::write(&log, "stub").unwrap();
            Ok(log)
        }
    }

    /// Store + engine + seeded job + seeded plan, ready for run_stage.
    fn pipeline_fixture(tmp: &std::path::Path) -> (Store, String, PathBuf) {
        let store = Store::new(tmp.to_path_buf());
        store.init_workspace().unwrap();
        let engine = LineageEngine::new(&store);
        let mut fb = mk_feedback();
        store.save_feedback(&fb).unwrap();
        let job = engine
            .enqueue_job_for_feedback(&mut fb, crate::types::AgentKind::default())
            .unwrap();
        engine.seed_stages(&job.id, StageKind::pipeline()).unwrap();
        let job_dir = tmp.join("job-dir");
        seed_plan_from_feedback(&store, &fb, &job.id, &job_dir, 1).unwrap();
        (store, job.id, job_dir)
    }

    #[test]
    fn run_stage_retries_with_failure_feedback_until_green() {
        let tmp = tempdir().unwrap();
        let (store, job_id, job_dir) = pipeline_fixture(tmp.path());
        let engine = LineageEngine::new(&store);
        let dispatcher = ScriptedDispatcher {
            job_dir: job_dir.clone(),
            fix_on_attempt: 2,
            calls: Default::default(),
        };
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(&worktree).unwrap();

        let passed = run_stage(
            &store,
            &engine,
            &dispatcher,
            StageKind::BackendPlan,
            &worktree,
            &job_dir,
            &job_id,
        )
        .unwrap();
        assert!(passed, "second attempt should validate green");

        let calls = dispatcher.calls.borrow();
        assert_eq!(calls.len(), 2, "one initial dispatch + one Reflexion retry");
        assert_eq!(calls[0].0, 1);
        assert!(calls[0].1.is_none(), "first attempt carries no failure");
        assert_eq!(calls[1].0, 2);
        let feedback = calls[1].1.as_deref().expect("retry carries the report");
        assert!(
            feedback.contains("FAILED"),
            "failure block lists failed checks: {feedback}"
        );
        assert!(
            feedback.contains("canvas.pngPath"),
            "the specific finding travels to the retry prompt: {feedback}"
        );

        let job = store.load_lineage_job(&job_id).unwrap().unwrap();
        let stage = job
            .stages
            .iter()
            .find(|s| s.kind == StageKind::BackendPlan)
            .unwrap();
        assert_eq!(stage.status, StageStatus::Green);
        let plan = load_plan(&job_dir).unwrap().unwrap();
        assert_eq!(plan.stage, PlanStage::BackendPlanned);
    }

    #[test]
    fn run_stage_gives_up_after_max_attempts() {
        let tmp = tempdir().unwrap();
        let (store, job_id, job_dir) = pipeline_fixture(tmp.path());
        let engine = LineageEngine::new(&store);
        let dispatcher = ScriptedDispatcher {
            job_dir: job_dir.clone(),
            fix_on_attempt: u32::MAX, // never fixes the plan
            calls: Default::default(),
        };
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(&worktree).unwrap();

        let passed = run_stage(
            &store,
            &engine,
            &dispatcher,
            StageKind::BackendPlan,
            &worktree,
            &job_dir,
            &job_id,
        )
        .unwrap();
        assert!(!passed);
        assert_eq!(
            dispatcher.calls.borrow().len() as u32,
            MAX_STAGE_ATTEMPTS,
            "every attempt dispatches, then the stage gives up"
        );
        let job = store.load_lineage_job(&job_id).unwrap().unwrap();
        let stage = job
            .stages
            .iter()
            .find(|s| s.kind == StageKind::BackendPlan)
            .unwrap();
        assert_eq!(stage.status, StageStatus::Failed);
    }

    #[test]
    fn run_stage_treats_unparseable_plan_as_failed_attempt_not_error() {
        let tmp = tempdir().unwrap();
        let (store, job_id, job_dir) = pipeline_fixture(tmp.path());
        let engine = LineageEngine::new(&store);

        /// Corrupts plan.json on every dispatch — simulates an agent that
        /// breaks the file. The stage must fail cleanly (with the parse
        /// error fed back), not abort the pipeline with Err.
        struct Corruptor {
            job_dir: PathBuf,
            calls: std::cell::RefCell<Vec<Option<String>>>,
        }
        impl StageDispatcher for Corruptor {
            fn dispatch(&self, ctx: StageDispatch<'_>) -> Result<PathBuf, String> {
                self.calls.borrow_mut().push(ctx.previous_failure.clone());
                std::fs::write(self.job_dir.join(crate::plan::PLAN_FILENAME), "{not json").unwrap();
                let log = self.job_dir.join("logs").join("x.log");
                std::fs::create_dir_all(log.parent().unwrap()).unwrap();
                std::fs::write(&log, "stub").unwrap();
                Ok(log)
            }
        }
        let dispatcher = Corruptor {
            job_dir: job_dir.clone(),
            calls: Default::default(),
        };
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(&worktree).unwrap();

        let passed = run_stage(
            &store,
            &engine,
            &dispatcher,
            StageKind::BackendPlan,
            &worktree,
            &job_dir,
            &job_id,
        )
        .unwrap();
        assert!(!passed);
        let calls = dispatcher.calls.borrow();
        assert_eq!(calls.len() as u32, MAX_STAGE_ATTEMPTS);
        let second = calls[1].as_deref().unwrap();
        assert!(
            second.contains("failed to parse"),
            "parse error is fed back to the retry: {second}"
        );
    }

    #[test]
    fn persist_stage_outcome_demotes_passed_report_when_plan_json_is_corrupt() {
        // Regression test for the `persist_stage_outcome` fix: a validator
        // can report `passed: true` (e.g. because `run_stage` already
        // loaded a good plan for `run_validator`), but if `plan.json` is
        // unreadable by the time `persist_stage_outcome` tries to reload
        // it to advance `plan.stage`, the stage must NOT be recorded
        // green — it must be demoted to failed with the reload error in
        // the detail, not silently marked Green with a frozen
        // `plan.stage`.
        let tmp = tempdir().unwrap();
        let (store, job_id, job_dir) = pipeline_fixture(tmp.path());
        let engine = LineageEngine::new(&store);

        // Corrupt plan.json out from under persist_stage_outcome.
        std::fs::write(job_dir.join(crate::plan::PLAN_FILENAME), "{not json").unwrap();

        let passing_report = crate::validators::StageReport {
            stage: StageKind::BackendPlan.slug().to_string(),
            passed: true,
            headline: "1 checks passed (1/1)".to_string(),
            checks: vec![crate::validators::CheckResult {
                name: "stub check".into(),
                passed: true,
                detail: String::new(),
            }],
            ran_at_unix_ms: 0,
        };

        let outcome = persist_stage_outcome(
            &store,
            &engine,
            &job_id,
            &job_dir,
            StageKind::BackendPlan,
            passing_report,
        )
        .unwrap();

        assert!(
            !outcome.passed,
            "a passing report must be demoted when plan.json can't be reloaded"
        );
        assert!(
            outcome
                .checks
                .iter()
                .any(|c| !c.passed && c.detail.contains("failed to reload")),
            "demotion reason should mention the reload failure: {:?}",
            outcome.checks
        );

        // The job record must reflect the demotion too — never left
        // saying Green while plan.stage silently failed to advance.
        let job = store.load_lineage_job(&job_id).unwrap().unwrap();
        let stage = job
            .stages
            .iter()
            .find(|s| s.kind == StageKind::BackendPlan)
            .unwrap();
        assert_eq!(stage.status, StageStatus::Failed);
    }

    #[test]
    fn run_stage_feeds_agent_declared_failure_into_retry_prompt() {
        let tmp = tempdir().unwrap();
        let (store, job_id, job_dir) = pipeline_fixture(tmp.path());
        let engine = LineageEngine::new(&store);

        // Exits 0 but writes a STAGE_RESULT: fail declaration into its log
        // and never completes the plan — the declared reason must reach the
        // next attempt's previous_failure.
        struct DeclaresFailure {
            job_dir: PathBuf,
            calls: std::cell::RefCell<Vec<Option<String>>>,
        }
        impl StageDispatcher for DeclaresFailure {
            fn dispatch(&self, ctx: StageDispatch<'_>) -> Result<PathBuf, String> {
                self.calls.borrow_mut().push(ctx.previous_failure.clone());
                let log = self.job_dir.join("logs").join("backend_plan.log");
                std::fs::create_dir_all(log.parent().unwrap()).unwrap();
                std::fs::write(&log, "...\nSTAGE_RESULT: fail: canvas png unreadable\n").unwrap();
                Ok(log)
            }
        }
        let dispatcher = DeclaresFailure {
            job_dir: job_dir.clone(),
            calls: Default::default(),
        };
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(&worktree).unwrap();

        let passed = run_stage(
            &store,
            &engine,
            &dispatcher,
            StageKind::BackendPlan,
            &worktree,
            &job_dir,
            &job_id,
        )
        .unwrap();
        assert!(!passed);
        let calls = dispatcher.calls.borrow();
        let second = calls[1].as_deref().unwrap();
        assert!(
            second.contains("agent self-declared failure: canvas png unreadable"),
            "declared reason missing from retry feedback: {second}"
        );
        let job = store.load_lineage_job(&job_id).unwrap().unwrap();
        assert!(
            job.notes
                .iter()
                .any(|n| n.contains("declared STAGE_RESULT: fail: canvas png unreadable")),
            "declared failure should be recorded as a lineage note"
        );
    }

    #[test]
    fn pipeline_pauses_after_backend_plan_and_resumes_after_approval() {
        let tmp = tempdir().unwrap();
        let (store, job_id, job_dir) = pipeline_fixture(tmp.path());
        let engine = LineageEngine::new(&store);
        engine.request_plan_review(&job_id).unwrap();

        // Dispatcher produces a green backend plan on the first attempt.
        let dispatcher = ScriptedDispatcher {
            job_dir: job_dir.clone(),
            fix_on_attempt: 1,
            calls: Default::default(),
        };
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(&worktree).unwrap();

        let run = run_pipeline(&store, &engine, &dispatcher, &worktree, &job_dir, &job_id).unwrap();
        assert!(run.paused_for_plan_review);
        let job = store.load_lineage_job(&job_id).unwrap().unwrap();
        assert_eq!(job.status, crate::types::LineageJobStatus::Planned);
        let backend_impl = job
            .stages
            .iter()
            .find(|s| s.kind == StageKind::BackendImpl)
            .unwrap();
        assert_eq!(
            backend_impl.status,
            StageStatus::Pending,
            "implementation must not have started"
        );
        assert_eq!(
            dispatcher.calls.borrow().len(),
            1,
            "only the BackendPlan stage dispatched"
        );

        // Approve → the gate never fires again; the pipeline proceeds into
        // BackendImpl (which red-flags on the empty worktree — fine, the
        // point is it RAN instead of re-pausing).
        engine.mark_plan_approved(&job_id).unwrap();
        let run = run_pipeline(&store, &engine, &dispatcher, &worktree, &job_dir, &job_id).unwrap();
        assert!(
            !run.paused_for_plan_review,
            "approved job must not re-pause"
        );
        let job = store.load_lineage_job(&job_id).unwrap().unwrap();
        let backend_impl = job
            .stages
            .iter()
            .find(|s| s.kind == StageKind::BackendImpl)
            .unwrap();
        assert_ne!(
            backend_impl.status,
            StageStatus::Pending,
            "BackendImpl must have been attempted after approval"
        );
    }

    #[test]
    fn simulated_assumption_forces_plan_review_until_approved() {
        let tmp = tempdir().unwrap();
        let (store, job_id, job_dir) = pipeline_fixture(tmp.path());
        let engine = LineageEngine::new(&store);
        // The job did NOT ask for plan review; the planner guesses the app
        // can be a simulation — that decision belongs to the user.
        let mut plan = load_plan(&job_dir).unwrap().unwrap();
        plan.open_questions
            .push("Agents are bookkeeping records only — assumed simulated?".into());
        save_plan(&job_dir, &plan).unwrap();

        let dispatcher = ScriptedDispatcher {
            job_dir: job_dir.clone(),
            fix_on_attempt: 1,
            calls: Default::default(),
        };
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(&worktree).unwrap();

        let run = run_pipeline(&store, &engine, &dispatcher, &worktree, &job_dir, &job_id).unwrap();
        assert!(run.paused_for_plan_review, "simulation guess must pause");
        let job = store.load_lineage_job(&job_id).unwrap().unwrap();
        assert_eq!(job.status, crate::types::LineageJobStatus::Planned);
        assert!(
            job.pause_after_plan,
            "approve path must treat it as a plan review"
        );
        assert!(job.notes.iter().any(|n| n.contains("plan review required")));

        engine.mark_plan_approved(&job_id).unwrap();
        let run = run_pipeline(&store, &engine, &dispatcher, &worktree, &job_dir, &job_id).unwrap();
        assert!(
            !run.paused_for_plan_review,
            "approved plan must not re-pause"
        );
    }

    #[test]
    fn answer_and_replan_continues_when_every_question_is_answered() {
        let tmp = tempdir().unwrap();
        let (store, job_id, job_dir) = pipeline_fixture(tmp.path());
        let engine = LineageEngine::new(&store);
        engine.request_plan_review(&job_id).unwrap();
        let mut plan = load_plan(&job_dir).unwrap().unwrap();
        plan.open_questions = vec!["Currency single? Assumed USD.".into()];
        save_plan(&job_dir, &plan).unwrap();

        let dispatcher = ScriptedDispatcher {
            job_dir: job_dir.clone(),
            fix_on_attempt: 1,
            calls: Default::default(),
        };
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(&worktree).unwrap();
        let run = run_pipeline(&store, &engine, &dispatcher, &worktree, &job_dir, &job_id).unwrap();
        assert!(run.paused_for_plan_review);

        // Reviewer answers and re-plans (what answer_plan_questions does).
        let mut plan = load_plan(&job_dir).unwrap().unwrap();
        crate::plan::record_answers(
            &mut plan,
            &[("Currency single? Assumed USD.".into(), "EUR".into())],
            1,
        );
        plan.stage = PlanStage::Seeded;
        save_plan(&job_dir, &plan).unwrap();
        let calls_before = dispatcher.calls.borrow().len();

        let run = run_pipeline(&store, &engine, &dispatcher, &worktree, &job_dir, &job_id).unwrap();
        assert!(
            dispatcher.calls.borrow().len() > calls_before,
            "planner re-dispatched"
        );
        assert!(
            !run.paused_for_plan_review,
            "all answered → no second pause"
        );
        let job = store.load_lineage_job(&job_id).unwrap().unwrap();
        assert!(job.plan_approved_at_unix_ms.is_some());
        assert!(job
            .notes
            .iter()
            .any(|n| n.contains("every open question is answered")));
    }

    #[test]
    fn answer_and_replan_pauses_again_when_a_question_is_left_open() {
        let tmp = tempdir().unwrap();
        let (store, job_id, job_dir) = pipeline_fixture(tmp.path());
        let engine = LineageEngine::new(&store);
        engine.request_plan_review(&job_id).unwrap();
        let mut plan = load_plan(&job_dir).unwrap().unwrap();
        plan.open_questions = vec!["Q1? Assumed A.".into(), "Q2? Assumed B.".into()];
        crate::plan::record_answers(&mut plan, &[("Q1? Assumed A.".into(), "C".into())], 1);
        save_plan(&job_dir, &plan).unwrap();
        let dispatcher = ScriptedDispatcher {
            job_dir: job_dir.clone(),
            fix_on_attempt: 1,
            calls: Default::default(),
        };
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(&worktree).unwrap();
        let run = run_pipeline(&store, &engine, &dispatcher, &worktree, &job_dir, &job_id).unwrap();
        assert!(run.paused_for_plan_review, "Q2 is still unanswered");
    }

    #[test]
    fn clean_plan_without_review_request_does_not_pause() {
        let tmp = tempdir().unwrap();
        let (store, job_id, job_dir) = pipeline_fixture(tmp.path());
        let engine = LineageEngine::new(&store);
        let dispatcher = ScriptedDispatcher {
            job_dir: job_dir.clone(),
            fix_on_attempt: 1,
            calls: Default::default(),
        };
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(&worktree).unwrap();
        let run = run_pipeline(&store, &engine, &dispatcher, &worktree, &job_dir, &job_id).unwrap();
        assert!(!run.paused_for_plan_review);
    }

    fn git(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
        std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap()
    }

    #[test]
    fn checkpoint_worktree_commits_dirty_state_and_skips_clean() {
        let tmp = tempdir().unwrap();
        let wt = tmp.path();
        assert!(git(wt, &["init"]).status.success());
        std::fs::write(wt.join("a.txt"), "v1").unwrap();

        let sha = checkpoint_worktree(wt, StageKind::BackendImpl, "job-9");
        assert!(sha.is_some(), "dirty worktree gets a checkpoint commit");
        let log = git(wt, &["log", "-1", "--format=%s %an"]);
        let line = String::from_utf8_lossy(&log.stdout);
        assert!(
            line.contains("stage(backend_impl): green checkpoint — job job-9"),
            "commit message: {line}"
        );
        assert!(
            line.contains("Evolvo Pipeline"),
            "pipeline identity: {line}"
        );

        // Clean tree → no checkpoint.
        assert!(checkpoint_worktree(wt, StageKind::FrontendImpl, "job-9").is_none());
    }

    #[test]
    fn checkpoint_worktree_is_noop_outside_git() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "v1").unwrap();
        assert!(checkpoint_worktree(tmp.path(), StageKind::BackendImpl, "job-9").is_none());
    }

    #[test]
    fn advance_plan_stage_is_monotonic() {
        // Resume re-validates upstream green stages; finishing BackendPlan
        // after the plan already reached FrontendImplemented must NOT
        // regress plan.stage — otherwise stage_already_done flips false
        // for downstream stages and the agent is re-dispatched against
        // artifacts that are already on disk.
        let mut plan = IterationPlan::default();
        plan.stage = PlanStage::FrontendImplemented;

        advance_plan_stage(&mut plan, StageKind::BackendPlan);
        assert_eq!(
            plan.stage,
            PlanStage::FrontendImplemented,
            "BackendPlan must not regress"
        );

        advance_plan_stage(&mut plan, StageKind::FrontendImpl);
        assert_eq!(
            plan.stage,
            PlanStage::FrontendImplemented,
            "same-ordinal must not bump"
        );

        advance_plan_stage(&mut plan, StageKind::E2EPlan);
        assert_eq!(
            plan.stage,
            PlanStage::E2EPlanned,
            "strictly-greater stage must advance"
        );

        advance_plan_stage(&mut plan, StageKind::FinalReview);
        assert_eq!(plan.stage, PlanStage::Completed);
    }
}
