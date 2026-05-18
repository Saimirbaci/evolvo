//! Deterministic, LLM-free validators that gate each stage of the NewApp
//! pipeline. A validator is the authoritative "done" signal — prompts are
//! advisory, the validator is not. If the validator rejects, the stage is
//! `Failed` regardless of what the Claude session claimed.
//!
//! Each validator returns a `StageReport` which is persisted verbatim into
//! the lineage job's `StageState.report` JSON blob, so the UI can render a
//! rich checklist without re-parsing free text.
//!
//! Philosophy:
//! - Checks are grep / cargo test / file-presence — nothing that needs an
//!   LLM. If the check isn't deterministic, it doesn't belong here.
//! - Reports are structured (`checks: Vec<CheckResult>`) so additions are
//!   forward-compatible.

use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::plan::IterationPlan;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StageReport {
    pub stage: String,
    pub passed: bool,
    pub headline: String,
    pub checks: Vec<CheckResult>,
    pub ran_at_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CheckResult {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

impl StageReport {
    fn new(stage: &str) -> Self {
        Self {
            stage: stage.to_string(),
            passed: false,
            headline: String::new(),
            checks: Vec::new(),
            ran_at_unix_ms: crate::types::current_time_unix_ms(),
        }
    }

    fn push<S: Into<String>>(&mut self, name: &str, passed: bool, detail: S) {
        self.checks.push(CheckResult {
            name: name.to_string(),
            passed,
            detail: detail.into(),
        });
    }

    fn finalize(mut self) -> Self {
        let fails: Vec<&CheckResult> = self.checks.iter().filter(|c| !c.passed).collect();
        self.passed = fails.is_empty();
        self.headline = if self.passed {
            format!(
                "{} checks passed ({}/{})",
                self.checks.len(),
                self.checks.len(),
                self.checks.len()
            )
        } else {
            format!(
                "{} of {} checks failed — first: {}",
                fails.len(),
                self.checks.len(),
                fails[0].name
            )
        };
        self
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }
}

/// --- Planner validators (read-only: inspect `plan.json` structure). ---

pub fn validate_backend_plan(plan: &IterationPlan) -> StageReport {
    let mut r = StageReport::new("backend_plan");

    r.push(
        "plan.schemaVersion is set",
        plan.schema_version > 0,
        format!("schemaVersion={}", plan.schema_version),
    );
    r.push(
        "app.name is set",
        !plan.app.name.trim().is_empty(),
        plan.app.name.clone(),
    );
    r.push(
        "canvas.pngPath is set",
        plan.canvas.png_path.is_some(),
        plan.canvas
            .png_path
            .clone()
            .unwrap_or_else(|| "<missing>".to_string()),
    );

    // Region-scaled floor — the planner can RAISE the budget in its
    // own output but can't fall below this. Recomputed here so a
    // planner that wrote `budget: { 1, 1, 1 }` still gets held to the
    // floor derived from the canvas.
    let floor = crate::plan::budget_floor_for_regions(plan.canvas.regions.len());
    let entity_floor = plan.backend.budget.min_entities.max(floor.min_entities);
    let command_floor = plan.backend.budget.min_commands.max(floor.min_commands);
    let test_floor = plan.backend.budget.min_tests.max(floor.min_tests);

    let entity_count = plan.backend.entities.len() as u32;
    r.push(
        "entities meet budget",
        entity_count >= entity_floor,
        format!(
            "have {} entities (min {}; floor for {} regions = {})",
            entity_count,
            entity_floor,
            plan.canvas.regions.len(),
            floor.min_entities
        ),
    );
    let cmd_count = plan.backend.commands.len() as u32;
    r.push(
        "commands meet budget",
        cmd_count >= command_floor,
        format!(
            "have {} commands (min {}; floor for {} regions = {})",
            cmd_count,
            command_floor,
            plan.canvas.regions.len(),
            floor.min_commands
        ),
    );
    let test_count = plan.backend.tests.len() as u32;
    r.push(
        "tests meet budget",
        test_count >= test_floor,
        format!(
            "have {} tests (min {}; floor for {} regions = {})",
            test_count,
            test_floor,
            plan.canvas.regions.len(),
            floor.min_tests
        ),
    );

    // Every command references a snake_case identifier.
    let cmd_names_ok = plan.backend.commands.iter().all(|c| is_snake_case(&c.name));
    r.push(
        "command names are snake_case",
        cmd_names_ok,
        if cmd_names_ok {
            "ok".to_string()
        } else {
            let bad: Vec<&str> = plan
                .backend
                .commands
                .iter()
                .filter(|c| !is_snake_case(&c.name))
                .map(|c| c.name.as_str())
                .collect();
            format!("non-snake: {:?}", bad)
        },
    );

    // Every command has a non-trivial body_sketch. The sketch forces
    // the planner to think through behaviour up front; the implementer
    // grepping against it (in validate_backend_impl) is what prevents
    // the trivial-stub failure mode.
    const MIN_SKETCH_BYTES: usize = 30;
    let thin_sketches: Vec<&str> = plan
        .backend
        .commands
        .iter()
        .filter(|c| c.body_sketch.trim().len() < MIN_SKETCH_BYTES)
        .map(|c| c.name.as_str())
        .collect();
    r.push(
        &format!(
            "every command has a body_sketch (>= {} chars)",
            MIN_SKETCH_BYTES
        ),
        thin_sketches.is_empty(),
        if thin_sketches.is_empty() {
            "ok".to_string()
        } else {
            format!("thin or missing body_sketch: {:?}", thin_sketches)
        },
    );

    // Every region has at least one backend owner when plans mention any.
    if !plan.canvas.regions.is_empty() {
        let owned: std::collections::HashSet<String> =
            crate::plan::owned_region_ids(plan).into_iter().collect();
        let missing: Vec<String> = plan
            .canvas
            .regions
            .iter()
            .map(|r| r.id.clone())
            .filter(|id| !owned.contains(id))
            .collect();
        r.push(
            "every canvas region has at least one backend owner",
            missing.is_empty(),
            if missing.is_empty() {
                "ok".to_string()
            } else {
                format!("orphan regions: {:?}", missing)
            },
        );
    }

    r.finalize()
}

pub fn validate_frontend_plan(plan: &IterationPlan) -> StageReport {
    let mut r = StageReport::new("frontend_plan");
    let route_count = plan.frontend.routes.len() as u32;
    r.push(
        "routes meet budget",
        route_count >= plan.frontend.budget.min_routes.max(1),
        format!(
            "have {} routes (min {})",
            route_count, plan.frontend.budget.min_routes
        ),
    );
    let comp_count = plan.frontend.components.len() as u32;
    r.push(
        "components meet budget",
        comp_count >= plan.frontend.budget.min_components.max(1),
        format!(
            "have {} components (min {})",
            comp_count, plan.frontend.budget.min_components
        ),
    );

    // Every command a route/component references must exist in the backend.
    let backend_cmds: std::collections::HashSet<&str> = plan
        .backend
        .commands
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    let mut missing: Vec<String> = Vec::new();
    for r_ in &plan.frontend.routes {
        for c in &r_.uses_commands {
            if !backend_cmds.contains(c.as_str()) {
                missing.push(format!("route {} → {}", r_.path, c));
            }
        }
    }
    for c in &plan.frontend.components {
        for cmd in &c.uses_commands {
            if !backend_cmds.contains(cmd.as_str()) {
                missing.push(format!("component {} → {}", c.name, cmd));
            }
        }
    }
    r.push(
        "every frontend command reference exists in backend.commands",
        missing.is_empty(),
        if missing.is_empty() {
            "ok".to_string()
        } else {
            format!("missing: {:?}", missing)
        },
    );
    r.finalize()
}

pub fn validate_e2e_plan(plan: &IterationPlan) -> StageReport {
    let mut r = StageReport::new("e2e_plan");
    r.push(
        "at least one scenario",
        !plan.e2e.scenarios.is_empty(),
        format!("{} scenarios", plan.e2e.scenarios.len()),
    );
    r.push(
        "persistence smoke declared",
        plan.e2e.persistence_smoke.is_some(),
        "required for every NewApp iteration",
    );
    r.finalize()
}

/// --- Impl validators (inspect worktree files + run cargo). ---

pub fn validate_backend_impl(plan: &IterationPlan, worktree: &Path) -> StageReport {
    let mut r = StageReport::new("backend_impl");
    let commands_rs = worktree.join("app/src-tauri/src/commands.rs");
    let lib_rs = worktree.join("app/src-tauri/src/lib.rs");
    let main_rs = worktree.join("app/src-tauri/src/main.rs");

    let commands_body = read(&commands_rs);
    let invoke_body = format!("{}\n{}", read(&lib_rs), read(&main_rs));

    for cmd in &plan.backend.commands {
        let has_def = commands_body.contains(&format!("fn {}", cmd.name));
        r.push(
            &format!("backend command `{}` defined in commands.rs", cmd.name),
            has_def,
            commands_rs.display().to_string(),
        );
        let has_registration = invoke_body.contains(&format!("commands::{}", cmd.name));
        r.push(
            &format!(
                "backend command `{}` registered in invoke_handler",
                cmd.name
            ),
            has_registration,
            "search lib.rs + main.rs",
        );

        // Anchor check — the planned body_sketch / anchors say what the
        // implementation should mention. We extract the function body
        // and require at least one anchor (or sketched identifier) to
        // appear in it. Catches the "compiles + named correctly but
        // doesn't actually do the work" failure mode.
        //
        // backend_impl.md prescribes a thin-wrapper pattern: the
        // `#[tauri::command]` `fn <name>(...)` delegates straight to a
        // plain `fn <name>_impl(&Store, ...)` so tests can call the
        // logic without the macro. The real work — and the anchored
        // identifiers — live in `_impl`. We therefore concatenate both
        // bodies and let either satisfy the anchor.
        if has_def {
            let anchors = effective_anchors(cmd);
            if !anchors.is_empty() {
                let wrapper_body =
                    extract_function_body(&commands_body, &cmd.name).unwrap_or_default();
                let impl_name = format!("{}_impl", cmd.name);
                let impl_body =
                    extract_function_body(&commands_body, &impl_name).unwrap_or_default();
                let combined = format!("{wrapper_body}\n{impl_body}");
                let hit = anchors.iter().any(|a| combined.contains(a.as_str()));
                r.push(
                    &format!(
                        "backend command `{}` body references an anchor",
                        cmd.name
                    ),
                    hit,
                    if hit {
                        "ok".to_string()
                    } else {
                        format!(
                            "neither `{}` nor `{}_impl` body mentions any of {:?} — possible shallow stub",
                            cmd.name, cmd.name, anchors
                        )
                    },
                );
            }
        }
    }

    // Store integration: for every planned entity, the host crate
    // must contain at least 3 of the 4 CRUD verbs scoped to the
    // entity. The check is tolerant of pluralization (we look for
    // `<verb>_<entity>` and `<verb>_<plural>`) and tolerant of where
    // the functions live — they can be in `store.rs`, a new
    // app-shape module, etc. The goal is just: persistence layer
    // exists, the commands aren't returning fake in-memory data.
    let host_src_files = collect_host_rs_files(worktree);
    let host_src_body = host_src_files
        .iter()
        .map(|p| read(p))
        .collect::<Vec<_>>()
        .join("\n");
    for entity in &plan.backend.entities {
        let names = expected_crud_fn_names(&entity.name);
        let mut hits: Vec<&str> = Vec::new();
        let mut misses: Vec<&str> = Vec::new();
        for verb in ["save", "load", "list", "delete"] {
            let candidates: Vec<&String> = names
                .iter()
                .filter(|n| n.starts_with(&format!("{verb}_")))
                .collect();
            let found = candidates
                .iter()
                .any(|n| host_src_body.contains(&format!("fn {}", n)));
            if found {
                hits.push(verb);
            } else {
                misses.push(verb);
            }
        }
        r.push(
            &format!(
                "entity `{}` has CRUD persistence (>= 3 of save/load/list/delete)",
                entity.name
            ),
            hits.len() >= 3,
            if hits.len() >= 3 {
                format!("present: {:?}", hits)
            } else {
                format!(
                    "missing CRUD verb(s) for `{}`: {:?} (looked for {:?})",
                    entity.name, misses, names
                )
            },
        );
    }

    // Every planned test must exist as a `fn <name>` in the worktree.
    let mut grep_tests_body = String::new();
    for rel in [
        "app/src-tauri/src/commands.rs",
        "app/src-tauri/src/store.rs",
        "app/src-tauri/src/lineage.rs",
        "app/src-tauri/src/runner.rs",
    ] {
        grep_tests_body.push_str(&read(&worktree.join(rel)));
    }
    for t in &plan.backend.tests {
        r.push(
            &format!("test `{}` exists", t.name),
            grep_tests_body.contains(&format!("fn {}", t.name)),
            t.module.clone(),
        );
    }

    // Stub smells — zero tolerance.
    r.push(
        "no TODO/unimplemented!/todo!() in new commands",
        !has_stub_smell(&commands_body),
        "grep for TODO / unimplemented! / todo!()",
    );

    // `cargo check -p evolvo_desktop` must pass — and so must `cargo test`.
    match run_cargo(worktree, &["check", "-p", "evolvo_desktop"]) {
        Ok(true) => r.push("cargo check -p evolvo_desktop", true, "ok"),
        Ok(false) => r.push("cargo check -p evolvo_desktop", false, "exited non-zero"),
        Err(e) => r.push("cargo check -p evolvo_desktop", false, e),
    }
    match run_cargo(worktree, &["test", "-p", "evolvo_desktop", "--lib"]) {
        Ok(true) => r.push("cargo test -p evolvo_desktop --lib", true, "ok"),
        Ok(false) => r.push(
            "cargo test -p evolvo_desktop --lib",
            false,
            "exited non-zero",
        ),
        Err(e) => r.push("cargo test -p evolvo_desktop --lib", false, e),
    }
    r.finalize()
}

/// Minimum bytes of new CSS the FrontendImpl stage must add on top of the
/// snapshot taken at pipeline start. 50 bytes is a loose floor — it rejects
/// "touched the file but added nothing" and "added one empty selector" while
/// leaving room for legitimately-small redesigns that mostly reuse classes.
pub const STYLES_MIN_GROWTH_BYTES: u64 = 50;

pub fn validate_frontend_impl(
    plan: &IterationPlan,
    worktree: &Path,
    job_dir: &Path,
) -> StageReport {
    let mut r = StageReport::new("frontend_impl");
    let interop_rs = read(&worktree.join("app/ui/src/interop.rs"));
    let app_rs = read(&worktree.join("app/ui/src/app.rs"));

    // Every backend command must have a mirrored interop wrapper.
    for cmd in &plan.backend.commands {
        r.push(
            &format!("interop wrapper for `{}`", cmd.name),
            interop_rs.contains(&format!("\"{}\"", cmd.name))
                || interop_rs.contains(&format!("invoke(\"{}", cmd.name)),
            "app/ui/src/interop.rs",
        );
    }

    for comp in &plan.frontend.components {
        let exists = app_rs.contains(&format!("fn {}", comp.name))
            || app_rs.contains(&format!("fn {}(", comp.name))
            || has_any_component(worktree, &comp.name);
        r.push(
            &format!("component `{}` exists in UI", comp.name),
            exists,
            comp.module.clone(),
        );
    }

    r.push(
        "no TODO/unimplemented!/todo!() in app.rs",
        !has_stub_smell(&app_rs),
        "grep for TODO / unimplemented! / todo!()",
    );

    // styles.css delta gate — the FrontendImpl stage must *add* CSS for the
    // NewApp's new components / routes. A previous iteration shipped a
    // behaviourally-correct app with zero styling because the model never
    // touched `app/ui/styles.css`. The snapshot taken at pipeline start lives
    // at `<job_dir>/inputs/styles.baseline.css`; we require the current file
    // to exist and to have grown by at least `STYLES_MIN_GROWTH_BYTES`.
    let styles_path = worktree.join("app/ui/styles.css");
    let baseline_path = job_dir
        .join("inputs")
        .join(crate::stages::STYLES_BASELINE_FILENAME);
    let styles_len = std::fs::metadata(&styles_path).map(|m| m.len()).ok();
    let baseline_len = std::fs::metadata(&baseline_path).map(|m| m.len()).ok();
    r.push(
        "app/ui/styles.css exists",
        styles_len.is_some(),
        styles_path.display().to_string(),
    );
    match (styles_len, baseline_len) {
        (Some(now), Some(base)) => {
            let grew_by = now.saturating_sub(base);
            r.push(
                "app/ui/styles.css grew vs baseline",
                grew_by >= STYLES_MIN_GROWTH_BYTES,
                format!(
                    "baseline={}B, current={}B, delta={}B (min {}B)",
                    base, now, grew_by, STYLES_MIN_GROWTH_BYTES
                ),
            );
        }
        (Some(_), None) => {
            // No baseline means the pipeline was launched before this gate
            // existed; skip the delta check rather than failing legacy jobs.
            r.push(
                "app/ui/styles.css grew vs baseline",
                true,
                "no baseline snapshot — skipped",
            );
        }
        _ => { /* first check already reported the missing file */ }
    }

    match run_cargo(
        worktree,
        &[
            "check",
            "-p",
            "evolvo_ui",
            "--target",
            "wasm32-unknown-unknown",
        ],
    ) {
        Ok(true) => r.push("cargo check -p evolvo_ui (wasm)", true, "ok"),
        Ok(false) => r.push("cargo check -p evolvo_ui (wasm)", false, "exited non-zero"),
        Err(e) => r.push("cargo check -p evolvo_ui (wasm)", false, e),
    }
    r.finalize()
}

pub fn validate_e2e_impl(plan: &IterationPlan, worktree: &Path) -> StageReport {
    let mut r = StageReport::new("e2e_impl");
    let scripts = worktree.join("scripts");
    r.push(
        "scripts/run-iteration.sh exists OR default stack intact",
        scripts.join("run-iteration.sh").exists() || worktree.join("app/src-tauri").exists(),
        scripts.display().to_string(),
    );
    if let Some(smoke) = &plan.e2e.persistence_smoke {
        r.push(
            "persistence_smoke.entity declared",
            !smoke.entity.trim().is_empty(),
            smoke.entity.clone(),
        );
    }
    r.push(
        "at least one scenario kept",
        !plan.e2e.scenarios.is_empty(),
        format!("{} scenarios", plan.e2e.scenarios.len()),
    );
    r.finalize()
}

/// --- Final review: aggregate gate across all prior stages + region
/// ownership completeness. Runs after `e2e_impl`. ---
pub fn validate_final(plan: &IterationPlan, worktree: &Path) -> StageReport {
    let mut r = StageReport::new("final_review");
    let owned: std::collections::HashSet<String> =
        crate::plan::owned_region_ids(plan).into_iter().collect();
    let missing: Vec<String> = plan
        .canvas
        .regions
        .iter()
        .map(|r| r.id.clone())
        .filter(|id| !owned.contains(id))
        .collect();
    r.push(
        "every canvas region has an owner somewhere",
        missing.is_empty(),
        if missing.is_empty() {
            "ok".to_string()
        } else {
            format!("orphans: {:?}", missing)
        },
    );
    r.push(
        "plan.stage marks pipeline complete",
        matches!(plan.stage, crate::plan::PlanStage::E2EImplemented)
            || matches!(plan.stage, crate::plan::PlanStage::Completed),
        format!("stage={}", plan.stage.label()),
    );
    // Iteration-1 NewApp runs MUST rewrite CLAUDE.md so the next
    // iteration's agent sees the new app's domain — not the host
    // Evolvo's shell. We check for unmistakable host fingerprints; the
    // check is intentionally narrow (false-positive cost is high — a
    // user might legitimately keep the Tauri stack and still mention
    // Evolvo, that's fine). The fingerprints we look for only appear
    // verbatim in the host's CLAUDE.md.
    //
    // Skipped for iteration ≥ 2: those iterations are bug fixes /
    // feature requests on an app that's already been documented; we
    // don't force a rewrite on every iteration.
    if plan.app.iteration <= 1 {
        let claude_md_path = worktree.join("CLAUDE.md");
        let (passed, detail) = check_claude_md_rewritten(&claude_md_path);
        r.push("CLAUDE.md rewritten for new app (iteration 1)", passed, detail);
    }
    r.finalize()
}

/// Stale-host-fingerprint check for iteration-1 NewApp runs. Returns
/// `(passed, detail)`. Passes (with a permissive detail string) if the
/// file is missing or empty — the check is a soft nudge, not a hard
/// gate, and a missing file is a different problem this validator
/// doesn't own.
fn check_claude_md_rewritten(path: &Path) -> (bool, String) {
    let body = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return (true, "no CLAUDE.md at worktree root (not enforced)".into()),
    };
    // Two fingerprints that only appear in the host Evolvo's CLAUDE.md
    // verbatim. Either survives a rewrite means the agent didn't
    // rewrite — they just appended.
    let host_header = "# Evolvo — Repository Guide for Claude";
    let host_one_liner = "Evolvo is a **Tauri 2** desktop app";
    let has_header = body.contains(host_header);
    let has_one_liner = body.contains(host_one_liner);
    if has_header && has_one_liner {
        (
            false,
            "still contains host Evolvo's exact header + one-liner — rewrite CLAUDE.md to describe the new app".into(),
        )
    } else if has_header {
        (
            false,
            "still contains the host Evolvo header line verbatim — replace it with the new app's name".into(),
        )
    } else {
        (true, "host fingerprints absent — looks rewritten".into())
    }
}

// --- helpers ---

fn is_snake_case(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !s.starts_with('_')
        && !s.ends_with('_')
}

/// Anchors to grep for inside a planned command's implementation body.
/// Prefers the explicit `anchors` list when the planner provided one;
/// otherwise falls back to identifier-shaped tokens extracted from the
/// `body_sketch`. We filter common stopwords / Rust keywords so the
/// extraction doesn't anchor on noise like "the", "and", "Self".
fn effective_anchors(cmd: &crate::plan::CommandPlan) -> Vec<String> {
    if !cmd.anchors.is_empty() {
        return cmd
            .anchors
            .iter()
            .map(|a| a.trim().to_string())
            .filter(|a| !a.is_empty())
            .collect();
    }
    extract_identifier_anchors(&cmd.body_sketch)
}

const ANCHOR_STOPWORDS: &[&str] = &[
    "and", "the", "for", "into", "from", "with", "return", "returns", "self", "Self", "store",
    "result", "ok", "err", "true", "false", "let", "mut", "fn", "pub", "use", "ref", "match",
    "if", "else", "loop", "while", "as", "in", "of", "to", "on", "this", "that", "then",
    "build", "make", "call", "id", "rec", "row", "row_id", "value", "string", "vec", "option",
];

fn extract_identifier_anchors(sketch: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let flush = |buf: &mut String, out: &mut Vec<String>| {
        if buf.is_empty() {
            return;
        }
        let lower = buf.to_ascii_lowercase();
        // Looks like a multi-word identifier — snake_case (has `_`) or
        // PascalCase / camelCase (has an uppercase letter at position
        // > 0). A single capitalised English word like "Generate" has
        // only its first letter uppercase and gets rejected.
        let looks_like_ident = buf.contains('_')
            || buf
                .chars()
                .enumerate()
                .any(|(i, c)| i > 0 && c.is_ascii_uppercase());
        let ok = buf.len() >= 5
            && !ANCHOR_STOPWORDS.contains(&lower.as_str())
            && !ANCHOR_STOPWORDS.contains(&buf.as_str())
            && looks_like_ident;
        if ok && !out.iter().any(|existing| existing == buf) {
            out.push(std::mem::take(buf));
        } else {
            buf.clear();
        }
    };
    for ch in sketch.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            current.push(ch);
        } else {
            flush(&mut current, &mut out);
        }
    }
    flush(&mut current, &mut out);
    out
}

/// Walk `<worktree>/app/src-tauri/src/` and collect every `*.rs` file
/// path. The CRUD integration check greps all of them so the agent
/// can put persistence in any module (e.g. a new app-shape under
/// `app_shapes/<name>.rs`) and still be credited.
fn collect_host_rs_files(worktree: &Path) -> Vec<std::path::PathBuf> {
    let root = worktree.join("app/src-tauri/src");
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let meta = match entry.file_type() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }
    out
}

/// Build the set of acceptable CRUD function names for an entity.
/// Tolerant of pluralization (English-ish): `Task → save_task,
/// load_task, list_tasks, delete_task` and `Story → list_stories`.
/// Both the singular and plural forms are returned for `list_*` so a
/// grep for either succeeds.
fn expected_crud_fn_names(entity: &str) -> Vec<String> {
    let snake = pascal_to_snake(entity);
    let plural = pluralize_snake(&snake);
    vec![
        format!("save_{snake}"),
        format!("load_{snake}"),
        format!("list_{plural}"),
        format!("list_{snake}"), // tolerant fallback
        format!("delete_{snake}"),
    ]
}

fn pascal_to_snake(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for (i, ch) in s.chars().enumerate() {
        if ch.is_ascii_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(ch.to_ascii_lowercase());
    }
    out
}

/// Simple English-ish pluralisation: `_y` (consonant + y) → `_ies`;
/// `_s|_x|_ch|_sh|_z` → `+es`; everything else → `+s`. Sufficient for
/// the common entity-name cases (Task → tasks, Entry → entries,
/// BudgetEntry → budget_entries, Box → boxes).
fn pluralize_snake(s: &str) -> String {
    if let Some(stem) = s.strip_suffix('y') {
        let prev = stem.chars().last();
        let is_vowel = matches!(prev, Some('a' | 'e' | 'i' | 'o' | 'u'));
        if !is_vowel {
            return format!("{stem}ies");
        }
    }
    if s.ends_with('s')
        || s.ends_with('x')
        || s.ends_with('z')
        || s.ends_with("ch")
        || s.ends_with("sh")
    {
        return format!("{s}es");
    }
    format!("{s}s")
}

/// Locate the body of a function named `name` inside `source` and
/// return its text (between the matching braces). Returns `None` when
/// the function isn't found or the braces don't balance. Coarse — we
/// don't tokenise Rust properly — but adequate for "does this body
/// mention identifier X" checks.
fn extract_function_body(source: &str, name: &str) -> Option<String> {
    let needle = format!("fn {}", name);
    let mut search_start = 0;
    loop {
        let idx = source[search_start..].find(&needle)?;
        let abs_idx = search_start + idx;
        // Avoid matching `fn create_taskish` when searching `create_task`.
        let after = &source[abs_idx + needle.len()..];
        let next_char = after.chars().next();
        let is_ident_cont = matches!(next_char, Some(c) if c == '_' || c.is_ascii_alphanumeric());
        if is_ident_cont {
            search_start = abs_idx + needle.len();
            continue;
        }
        let brace_idx = source[abs_idx..].find('{')?;
        let body_start = abs_idx + brace_idx + 1;
        let bytes = source.as_bytes();
        let mut depth = 1i32;
        let mut i = body_start;
        while i < bytes.len() {
            match bytes[i] {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(source[body_start..i].to_string());
                    }
                }
                _ => {}
            }
            i += 1;
        }
        return None;
    }
}

/// Pattern-based stub detection. Catches the obvious "agent named a
/// function but didn't implement it" shapes:
///
/// - `TODO`, `unimplemented!`, `todo!()` — historic patterns the
///   agent has been instructed to avoid for years.
/// - `Err("not impl…")`, `Err("nyi…")`, `Err("todo…")`, `Err("…not
///   implemented…")` — common bail-out shapes that compile fine.
/// - `panic!("…not implemented…")` / `panic!("…nyi…")` / `panic!("…not
///   yet…")` — sometimes used in place of `unimplemented!`.
/// - Trailing-expression returns of empty defaults at the end of a
///   function body: `Ok(Vec::new())` followed by `}`, `Ok(vec![])` →
///   `}`, `Ok(Default::default())` → `}`. These compile and pass
///   `cargo test` but mean the implementation never touched the store.
///
/// The trailing-default check is intentionally narrow: it requires the
/// pattern to be the final expression before a closing brace, with
/// only whitespace / optional comments in between. A legitimate "empty
/// list when directory is missing" case lives inside `store.rs`, not
/// `commands.rs` — `commands.rs` should always route through `Store`.
fn has_stub_smell(body: &str) -> bool {
    if body.contains("TODO") || body.contains("unimplemented!") || body.contains("todo!()") {
        return true;
    }
    // String-shaped bail-outs. Case-insensitive lookup on `not impl`
    // would over-match (e.g. "not implication"). We pin the patterns
    // to specific phrases known to appear in lazy stubs.
    for phrase in &[
        "Err(\"not impl",
        "Err(\"not yet",
        "Err(\"nyi",
        "Err(\"NYI",
        "Err(\"todo",
        "Err(\"TODO",
        "Err(\"not implemented",
        "panic!(\"not impl",
        "panic!(\"not yet",
        "panic!(\"nyi",
        "panic!(\"todo",
    ] {
        if body.contains(*phrase) {
            return true;
        }
    }
    // Trailing-default returns. Look for the pattern <default> } where
    // <default> is one of the three known shapes and only whitespace
    // (no semicolon, no other expression) sits between it and the `}`.
    for default in &[
        "Ok(Vec::new())",
        "Ok(vec![])",
        "Ok(Default::default())",
        "Ok(HashMap::new())",
        "Ok(BTreeMap::new())",
        "Ok(HashSet::new())",
    ] {
        let mut start = 0;
        while let Some(rel) = body[start..].find(default) {
            let pos = start + rel + default.len();
            // Walk forward past whitespace; if we hit `}` before any
            // other non-whitespace character, this is a trailing return.
            let rest = body[pos..].as_bytes();
            let mut i = 0;
            while i < rest.len() && (rest[i] == b' ' || rest[i] == b'\n' || rest[i] == b'\t') {
                i += 1;
            }
            if i < rest.len() && rest[i] == b'}' {
                return true;
            }
            start = pos;
        }
    }
    false
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

/// True iff `name` appears as a `#[component]` / `fn <name>` in any .rs
/// file under `app/ui/src/`.
fn has_any_component(worktree: &Path, name: &str) -> bool {
    let root = worktree.join("app/ui/src");
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let body = std::fs::read_to_string(&path).unwrap_or_default();
            if body.contains(&format!("fn {name}")) {
                return true;
            }
        }
    }
    false
}

fn run_cargo(worktree: &Path, args: &[&str]) -> Result<bool, String> {
    let out = Command::new("cargo")
        .args(args)
        .current_dir(worktree)
        .output()
        .map_err(|e| format!("spawn cargo: {e}"))?;
    Ok(out.status.success())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{
        AppIdentity, BackendBudget, BackendSection, CanvasReference, CanvasRegion, CommandPlan,
        EntityPlan, FieldPlan, FrontendBudget, FrontendSection, IterationPlan, PlanStage, TestPlan,
    };

    fn seed_plan() -> IterationPlan {
        IterationPlan {
            schema_version: 1,
            app: AppIdentity {
                name: "Demo".into(),
                ..Default::default()
            },
            stage: PlanStage::Seeded,
            canvas: CanvasReference {
                png_path: Some("/tmp/canvas.png".into()),
                regions: vec![CanvasRegion {
                    id: "R1".into(),
                    bbox: [0.0, 0.0, 10.0, 10.0],
                    stroke_count: 1,
                    dominant_color: None,
                    labels: vec![],
                }],
                ..Default::default()
            },
            backend: BackendSection {
                entities: vec![EntityPlan {
                    name: "Project".into(),
                    fields: vec![FieldPlan {
                        name: "id".into(),
                        ty: "String".into(),
                        required: true,
                    }],
                    motivated_by_regions: vec!["R1".into()],
                }],
                commands: vec![
                    CommandPlan {
                        name: "create_project".into(),
                        input: "()".into(),
                        output: "Project".into(),
                        motivated_by_regions: vec!["R1".into()],
                        summary: "".into(),
                        body_sketch:
                            "Generate id, build Project, save via store.save_project(rec)."
                                .into(),
                        anchors: vec!["save_project".into()],
                    },
                    CommandPlan {
                        name: "list_projects".into(),
                        input: "()".into(),
                        output: "Vec<Project>".into(),
                        motivated_by_regions: vec!["R1".into()],
                        summary: "".into(),
                        body_sketch: "Call store.list_projects() and return.".into(),
                        anchors: vec!["list_projects".into()],
                    },
                    CommandPlan {
                        name: "get_project".into(),
                        input: "EntityIdPayload".into(),
                        output: "Option<Project>".into(),
                        motivated_by_regions: vec!["R1".into()],
                        summary: "".into(),
                        body_sketch: "Resolve id, return store.load_project(id).".into(),
                        anchors: vec!["load_project".into()],
                    },
                    CommandPlan {
                        name: "delete_project".into(),
                        input: "EntityIdPayload".into(),
                        output: "bool".into(),
                        motivated_by_regions: vec!["R1".into()],
                        summary: "".into(),
                        body_sketch: "Resolve id, call store.delete_project(id), return bool."
                            .into(),
                        anchors: vec!["delete_project".into()],
                    },
                ],
                tests: vec![
                    TestPlan {
                        name: "roundtrip".into(),
                        module: "store::tests".into(),
                        covers: "save+load".into(),
                    },
                    TestPlan {
                        name: "create_and_list".into(),
                        module: "commands::tests".into(),
                        covers: "happy path".into(),
                    },
                    TestPlan {
                        name: "delete_returns_false_when_missing".into(),
                        module: "commands::tests".into(),
                        covers: "edge".into(),
                    },
                ],
                storage: Default::default(),
                budget: BackendBudget {
                    min_entities: 1,
                    min_commands: 4,
                    min_tests: 3,
                },
            },
            frontend: FrontendSection {
                budget: FrontendBudget {
                    min_routes: 1,
                    min_components: 1,
                },
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn backend_plan_validator_passes_on_minimum() {
        let plan = seed_plan();
        let report = validate_backend_plan(&plan);
        assert!(report.passed, "{:#?}", report);
    }

    #[test]
    fn backend_plan_validator_rejects_orphan_region() {
        let mut plan = seed_plan();
        plan.backend.entities[0].motivated_by_regions.clear();
        for c in &mut plan.backend.commands {
            c.motivated_by_regions.clear();
        }
        let report = validate_backend_plan(&plan);
        assert!(!report.passed);
        assert!(report.checks.iter().any(|c| c
            .name
            .contains("canvas region has at least one backend owner")
            && !c.passed));
    }

    #[test]
    fn backend_plan_validator_rejects_bad_command_name() {
        let mut plan = seed_plan();
        plan.backend.commands[0].name = "CreateProject".into();
        let report = validate_backend_plan(&plan);
        assert!(!report.passed);
    }

    #[test]
    fn frontend_plan_validator_rejects_unknown_command_ref() {
        let mut plan = seed_plan();
        plan.frontend.routes.push(crate::plan::RoutePlan {
            path: "/".into(),
            component: "Home".into(),
            uses_commands: vec!["does_not_exist".into()],
            motivated_by_regions: vec![],
        });
        plan.frontend.components.push(crate::plan::ComponentPlan {
            name: "Home".into(),
            module: "app.rs".into(),
            uses_commands: vec![],
            summary: "".into(),
            motivated_by_regions: vec!["R1".into()],
        });
        plan.frontend.components.push(crate::plan::ComponentPlan {
            name: "Nav".into(),
            module: "app.rs".into(),
            uses_commands: vec![],
            summary: "".into(),
            motivated_by_regions: vec![],
        });
        let report = validate_frontend_plan(&plan);
        assert!(!report.passed);
    }

    #[test]
    fn snake_case_helper_rejects_camel_and_hyphen() {
        assert!(is_snake_case("hello_world"));
        assert!(is_snake_case("cmd1"));
        assert!(!is_snake_case("helloWorld"));
        assert!(!is_snake_case("_leading"));
        assert!(!is_snake_case("trailing_"));
        assert!(!is_snake_case(""));
    }

    #[test]
    fn stub_smell_detects_common_markers() {
        assert!(has_stub_smell("let x = todo!();"));
        assert!(has_stub_smell("// TODO: hook this up"));
        assert!(has_stub_smell("unimplemented!();"));
        assert!(!has_stub_smell("let x = 1;"));
    }

    #[test]
    fn stub_smell_catches_err_bailouts() {
        assert!(has_stub_smell(r#"return Err("not impl".into());"#));
        assert!(has_stub_smell(r#"Err("nyi".into())"#));
        assert!(has_stub_smell(r#"Err("not implemented yet".into())"#));
        assert!(has_stub_smell(r#"panic!("not impl");"#));
        assert!(has_stub_smell(r#"panic!("nyi");"#));
        // Not a bail-out — legitimate error string.
        assert!(!has_stub_smell(r#"Err(format!("could not parse: {e}"))"#));
    }

    #[test]
    fn stub_smell_catches_trailing_empty_defaults() {
        // Trailing return of an empty container — most common shallow
        // pattern.
        let body = "fn list_tasks() -> Result<Vec<Task>, String> {\n    Ok(Vec::new())\n}";
        assert!(has_stub_smell(body));
        let body = "fn list_x() -> Result<Vec<u32>, String> {\n    Ok(vec![])\n}";
        assert!(has_stub_smell(body));
        let body = "fn get_thing() -> Result<Thing, String> {\n    Ok(Default::default())\n}";
        assert!(has_stub_smell(body));
    }

    #[test]
    fn stub_smell_allows_legitimate_intermediate_empties() {
        // An empty vec used as an intermediate value (not a trailing
        // return) is fine — common in real code.
        let body = r#"
            let mut acc: Vec<Task> = Ok(Vec::new())?;
            acc.push(t);
            Ok(acc)
        "#;
        assert!(!has_stub_smell(body));
    }

    #[test]
    fn extract_function_body_grabs_balanced_braces() {
        let src = r#"
            pub fn create_task(state: State<'_>) -> Result<Task, String> {
                let id = format!("task-{}", now());
                let rec = Task { id: id.clone() };
                state.store().save_task(&rec)?;
                Ok(rec)
            }

            pub fn unrelated() -> i32 { 7 }
        "#;
        let body = extract_function_body(src, "create_task").unwrap();
        assert!(body.contains("save_task"));
        assert!(body.contains("format!"));
        assert!(!body.contains("unrelated"));
    }

    #[test]
    fn extract_function_body_skips_partial_name_matches() {
        let src = r#"
            fn create_taskish() -> i32 { 1 }
            fn create_task() -> i32 { 42 }
        "#;
        let body = extract_function_body(src, "create_task").unwrap();
        assert!(body.contains("42"));
        assert!(!body.contains("1"));
    }

    #[test]
    fn extract_identifier_anchors_picks_likely_idents() {
        let sketch = "Generate id with current_time_unix_ms, then call store.save_task and return.";
        let anchors = extract_identifier_anchors(sketch);
        assert!(anchors.contains(&"current_time_unix_ms".to_string()));
        assert!(anchors.contains(&"save_task".to_string()));
        // Common stopwords (`and`, `the`, `then`, `Generate`) shouldn't anchor.
        assert!(!anchors.iter().any(|a| a == "Generate"));
        assert!(!anchors.iter().any(|a| a == "and"));
    }

    #[test]
    fn validate_backend_impl_credits_anchors_found_in_impl_function() {
        // Regression: the thin-wrapper pattern that backend_impl.md
        // prescribes — `fn <name>(...)` delegates to `fn <name>_impl(...)`
        // where the real work lives. The wrapper body contains zero
        // anchors; the validator must also look at `_impl` and accept
        // a match there.
        use crate::plan::{
            AppIdentity, BackendSection, CommandPlan, EntityPlan, FieldPlan, IterationPlan,
            PlanStage, PLAN_SCHEMA_VERSION,
        };
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path();
        let src_dir = worktree.join("app/src-tauri/src");
        std::fs::create_dir_all(&src_dir).unwrap();
        // commands.rs: thin wrapper + _impl, exactly as the prompt
        // tells the agent to ship.
        std::fs::write(
            src_dir.join("commands.rs"),
            r#"
                pub fn create_expense_impl(
                    store: &crate::store::Store,
                    payload: CreateExpensePayload,
                ) -> Result<Expense, String> {
                    let now = current_time_unix_ms();
                    let rec = Expense { id: format!("exp-{now}") };
                    store.save_expense(&rec).map_err(store_error)?;
                    Ok(rec)
                }

                #[tauri::command]
                pub fn create_expense(
                    state: State<'_, AppState>,
                    payload: CreateExpensePayload,
                ) -> Result<Expense, String> {
                    create_expense_impl(&state.store(), payload)
                }
            "#,
        )
        .unwrap();
        // Need a lib.rs / main.rs so the registration check finds a
        // `commands::create_expense` reference.
        std::fs::write(
            src_dir.join("lib.rs"),
            "invoke_handler![commands::create_expense];",
        )
        .unwrap();
        std::fs::write(src_dir.join("main.rs"), "// no-op").unwrap();
        // Store-side CRUD: satisfy the CRUD-presence check too.
        std::fs::write(
            src_dir.join("store.rs"),
            r#"
                pub fn save_expense() {}
                pub fn load_expense() {}
                pub fn list_expenses() {}
                pub fn delete_expense() {}
            "#,
        )
        .unwrap();

        let plan = IterationPlan {
            schema_version: PLAN_SCHEMA_VERSION,
            app: AppIdentity {
                name: "Ledger".into(),
                ..Default::default()
            },
            stage: PlanStage::Seeded,
            backend: BackendSection {
                entities: vec![EntityPlan {
                    name: "Expense".into(),
                    fields: vec![FieldPlan {
                        name: "id".into(),
                        ty: "String".into(),
                        required: true,
                    }],
                    motivated_by_regions: vec![],
                }],
                commands: vec![CommandPlan {
                    name: "create_expense".into(),
                    input: "CreateExpensePayload".into(),
                    output: "Expense".into(),
                    motivated_by_regions: vec![],
                    summary: "Persist an expense.".into(),
                    body_sketch: "Generate id, build Expense, persist via store.save_expense, return rec."
                        .into(),
                    anchors: vec!["save_expense".into(), "current_time_unix_ms".into()],
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        // We can't run the full validate_backend_impl here because it
        // shells out to `cargo` and that path is the host repo, not
        // the tempdir. Hit the anchor logic directly via the same
        // primitives.
        let commands_body = std::fs::read_to_string(src_dir.join("commands.rs")).unwrap();
        let anchors = effective_anchors(&plan.backend.commands[0]);
        assert!(anchors.contains(&"save_expense".to_string()));
        let wrapper_body =
            extract_function_body(&commands_body, "create_expense").unwrap_or_default();
        let impl_body =
            extract_function_body(&commands_body, "create_expense_impl").unwrap_or_default();
        let combined = format!("{wrapper_body}\n{impl_body}");
        let hit = anchors.iter().any(|a| combined.contains(a.as_str()));
        assert!(
            hit,
            "expected an anchor to be found in wrapper+_impl bodies; wrapper={:?} impl={:?}",
            wrapper_body, impl_body
        );
        // Sanity: anchors are NOT in the wrapper alone — that's the
        // condition that produced the original false positive.
        let wrapper_only_hit = anchors.iter().any(|a| wrapper_body.contains(a.as_str()));
        assert!(!wrapper_only_hit, "regression: wrapper alone now contains an anchor — test premise broken");
    }

    #[test]
    fn pluralize_snake_handles_y_and_s_endings() {
        assert_eq!(pluralize_snake("task"), "tasks");
        assert_eq!(pluralize_snake("entry"), "entries");
        assert_eq!(pluralize_snake("budget_entry"), "budget_entries");
        assert_eq!(pluralize_snake("box"), "boxes");
        assert_eq!(pluralize_snake("status"), "statuses");
        assert_eq!(pluralize_snake("day"), "days"); // vowel before y → keep y
    }

    #[test]
    fn expected_crud_fn_names_for_camel_cased_entity() {
        let names = expected_crud_fn_names("BudgetEntry");
        assert!(names.iter().any(|n| n == "save_budget_entry"));
        assert!(names.iter().any(|n| n == "list_budget_entries"));
        assert!(names.iter().any(|n| n == "delete_budget_entry"));
    }

    #[test]
    fn budget_floor_scales_with_region_count() {
        let f0 = crate::plan::budget_floor_for_regions(0);
        assert_eq!(f0.min_commands, 4);
        assert_eq!(f0.min_entities, 1);
        assert_eq!(f0.min_tests, 3);

        let f5 = crate::plan::budget_floor_for_regions(5);
        assert_eq!(f5.min_commands, 10); // 5 * 2
        assert_eq!(f5.min_entities, 2);  // 5 / 2
        assert_eq!(f5.min_tests, 5);
    }

    #[test]
    fn validate_backend_plan_enforces_region_floor_even_when_budget_low() {
        use crate::plan::{
            AppIdentity, BackendBudget, BackendSection, CanvasReference, CanvasRegion,
            CommandPlan, EntityPlan, FieldPlan, IterationPlan, PlanStage, TestPlan,
            PLAN_SCHEMA_VERSION,
        };
        let mut plan = IterationPlan {
            schema_version: PLAN_SCHEMA_VERSION,
            app: AppIdentity {
                name: "Demo".into(),
                ..Default::default()
            },
            stage: PlanStage::Seeded,
            canvas: CanvasReference {
                regions: (0..5)
                    .map(|i| CanvasRegion {
                        id: format!("R{}", i + 1),
                        bbox: [0.0; 4],
                        stroke_count: 0,
                        dominant_color: None,
                        labels: Vec::new(),
                    })
                    .collect(),
                ..Default::default()
            },
            backend: BackendSection {
                budget: BackendBudget {
                    min_entities: 1,
                    min_commands: 1, // planner tried to lower the bar
                    min_tests: 1,
                },
                entities: vec![EntityPlan {
                    name: "Task".into(),
                    fields: vec![FieldPlan {
                        name: "id".into(),
                        ty: "String".into(),
                        required: true,
                    }],
                    motivated_by_regions: vec!["R1".into(), "R2".into(), "R3".into(), "R4".into(), "R5".into()],
                }],
                commands: vec![CommandPlan {
                    name: "list_tasks".into(),
                    input: "()".into(),
                    output: "Vec<Task>".into(),
                    motivated_by_regions: vec!["R1".into(), "R2".into(), "R3".into(), "R4".into(), "R5".into()],
                    summary: "".into(),
                    body_sketch: "Call store.list_tasks() and return the vector.".into(),
                    anchors: vec!["list_tasks".into()],
                }],
                tests: vec![TestPlan {
                    name: "t".into(),
                    module: "".into(),
                    covers: "".into(),
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let r = validate_backend_plan(&plan);
        // The plan declared min_commands=1 but the canvas has 5
        // regions so the floor is 10. Should fail "commands meet
        // budget".
        let cmd_check = r
            .checks
            .iter()
            .find(|c| c.name == "commands meet budget")
            .unwrap();
        assert!(!cmd_check.passed, "should fail when below region floor");
        assert!(cmd_check.detail.contains("floor for 5 regions = 10"));

        // Now bump commands to 10 — should pass.
        for i in 0..9 {
            plan.backend.commands.push(CommandPlan {
                name: format!("filler_cmd_{i}"),
                input: "()".into(),
                output: "()".into(),
                motivated_by_regions: vec!["R1".into()],
                summary: "".into(),
                body_sketch: "Filler command for the test; calls store.list_tasks.".into(),
                anchors: vec!["list_tasks".into()],
            });
        }
        let r = validate_backend_plan(&plan);
        let cmd_check = r
            .checks
            .iter()
            .find(|c| c.name == "commands meet budget")
            .unwrap();
        assert!(cmd_check.passed, "10 commands meets the 5-region floor");
    }

    #[test]
    fn validate_backend_plan_requires_body_sketch() {
        use crate::plan::{
            AppIdentity, BackendSection, CommandPlan, IterationPlan, PlanStage,
            PLAN_SCHEMA_VERSION,
        };
        let plan = IterationPlan {
            schema_version: PLAN_SCHEMA_VERSION,
            app: AppIdentity {
                name: "Demo".into(),
                ..Default::default()
            },
            stage: PlanStage::Seeded,
            backend: BackendSection {
                commands: vec![CommandPlan {
                    name: "create_task".into(),
                    input: "()".into(),
                    output: "Task".into(),
                    motivated_by_regions: vec![],
                    summary: "creates".into(),
                    body_sketch: "too short".into(), // < 30 chars
                    anchors: vec![],
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let r = validate_backend_plan(&plan);
        let sketch_check = r
            .checks
            .iter()
            .find(|c| c.name.contains("body_sketch"))
            .unwrap();
        assert!(!sketch_check.passed);
        assert!(sketch_check.detail.contains("create_task"));
    }

    #[test]
    fn stage_report_finalize_sets_passed_when_all_green() {
        let mut r = StageReport::new("t");
        r.push("a", true, "ok");
        r.push("b", true, "ok");
        let r = r.finalize();
        assert!(r.passed);
        assert!(r.headline.contains("passed"));
    }

    #[test]
    fn frontend_impl_rejects_untouched_styles() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path().join("wt");
        let job_dir = tmp.path().join("job");
        let ui_src = worktree.join("app/ui/src");
        std::fs::create_dir_all(&ui_src).unwrap();
        std::fs::write(ui_src.join("interop.rs"), "").unwrap();
        std::fs::write(ui_src.join("app.rs"), "fn Home() {}\n").unwrap();

        let styles_path = worktree.join("app/ui/styles.css");
        let original = "body { color: black; }\n".repeat(20);
        std::fs::create_dir_all(styles_path.parent().unwrap()).unwrap();
        std::fs::write(&styles_path, &original).unwrap();

        let inputs = job_dir.join("inputs");
        std::fs::create_dir_all(&inputs).unwrap();
        std::fs::write(
            inputs.join(crate::stages::STYLES_BASELINE_FILENAME),
            &original,
        )
        .unwrap();

        let mut plan = seed_plan();
        plan.frontend.components.push(crate::plan::ComponentPlan {
            name: "Home".into(),
            module: "app.rs".into(),
            uses_commands: vec![],
            summary: "".into(),
            motivated_by_regions: vec![],
        });
        let report = validate_frontend_impl(&plan, &worktree, &job_dir);
        let styles_check = report
            .checks
            .iter()
            .find(|c| c.name == "app/ui/styles.css grew vs baseline")
            .expect("styles growth check present");
        assert!(
            !styles_check.passed,
            "untouched styles.css must fail the delta gate: {:?}",
            styles_check
        );
    }

    #[test]
    fn frontend_impl_accepts_grown_styles() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path().join("wt");
        let job_dir = tmp.path().join("job");
        let ui_src = worktree.join("app/ui/src");
        std::fs::create_dir_all(&ui_src).unwrap();
        std::fs::write(ui_src.join("interop.rs"), "").unwrap();
        std::fs::write(ui_src.join("app.rs"), "fn Home() {}\n").unwrap();

        let styles_path = worktree.join("app/ui/styles.css");
        let baseline = "body { color: black; }\n".to_string();
        let current = format!(
            "{}\n.home-card {{ padding: 16px; border-radius: 8px; background: #fff; }}\n",
            baseline
        );
        std::fs::create_dir_all(styles_path.parent().unwrap()).unwrap();
        std::fs::write(&styles_path, &current).unwrap();

        let inputs = job_dir.join("inputs");
        std::fs::create_dir_all(&inputs).unwrap();
        std::fs::write(
            inputs.join(crate::stages::STYLES_BASELINE_FILENAME),
            &baseline,
        )
        .unwrap();

        let mut plan = seed_plan();
        plan.frontend.components.push(crate::plan::ComponentPlan {
            name: "Home".into(),
            module: "app.rs".into(),
            uses_commands: vec![],
            summary: "".into(),
            motivated_by_regions: vec![],
        });
        let report = validate_frontend_impl(&plan, &worktree, &job_dir);
        let styles_check = report
            .checks
            .iter()
            .find(|c| c.name == "app/ui/styles.css grew vs baseline")
            .expect("styles growth check present");
        assert!(
            styles_check.passed,
            "grown styles.css must pass the delta gate: {:?}",
            styles_check
        );
    }

    #[test]
    fn stage_report_finalize_is_failed_on_any_red() {
        let mut r = StageReport::new("t");
        r.push("a", true, "ok");
        r.push("b", false, "nope");
        let r = r.finalize();
        assert!(!r.passed);
        assert!(r.headline.contains("failed"));
    }

    #[test]
    fn final_review_fails_when_claude_md_still_has_host_fingerprint_on_iter_1() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path();
        // The host Evolvo's exact header + one-liner survived intact.
        std::fs::write(
            worktree.join("CLAUDE.md"),
            "# Evolvo — Repository Guide for Claude\n\
             \n\
             Evolvo is a **Tauri 2** desktop app with a Leptos frontend.\n",
        )
        .unwrap();

        let mut plan = crate::plan::IterationPlan::default();
        plan.app.iteration = 1;
        plan.stage = crate::plan::PlanStage::E2EImplemented;
        let report = validate_final(&plan, worktree);
        let rewrite_check = report
            .checks
            .iter()
            .find(|c| c.name.contains("CLAUDE.md rewritten"))
            .expect("rewrite check should exist for iteration 1");
        assert!(!rewrite_check.passed, "should flag stale host fingerprints");
        assert!(rewrite_check.detail.contains("host Evolvo"));
    }

    #[test]
    fn final_review_passes_rewrite_check_when_claude_md_describes_new_app() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path();
        std::fs::write(
            worktree.join("CLAUDE.md"),
            "# Tasky — local-first todo app\n\
             \n\
             Tasky is a single-user task tracker built on the Evolvo shell.\n",
        )
        .unwrap();

        let mut plan = crate::plan::IterationPlan::default();
        plan.app.iteration = 1;
        plan.stage = crate::plan::PlanStage::E2EImplemented;
        let report = validate_final(&plan, worktree);
        let rewrite_check = report
            .checks
            .iter()
            .find(|c| c.name.contains("CLAUDE.md rewritten"))
            .expect("rewrite check should run on iteration 1");
        assert!(rewrite_check.passed, "should accept a rewritten file");
    }

    #[test]
    fn final_review_skips_rewrite_check_on_iteration_2() {
        // Iteration 2+ is a bug fix on an already-documented app —
        // we don't force a rewrite. CLAUDE.md can keep referencing the
        // (already-correctly-named) app from iteration 1.
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path();
        std::fs::write(
            worktree.join("CLAUDE.md"),
            "# Evolvo — Repository Guide for Claude\n\nEvolvo is a **Tauri 2** desktop app",
        )
        .unwrap();
        let mut plan = crate::plan::IterationPlan::default();
        plan.app.iteration = 2;
        plan.stage = crate::plan::PlanStage::E2EImplemented;
        let report = validate_final(&plan, worktree);
        assert!(
            !report
                .checks
                .iter()
                .any(|c| c.name.contains("CLAUDE.md rewritten")),
            "rewrite check should not fire on iteration 2+"
        );
    }
}
