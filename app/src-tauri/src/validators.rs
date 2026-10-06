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
    pub(crate) fn new(stage: &str) -> Self {
        Self {
            stage: stage.to_string(),
            passed: false,
            headline: String::new(),
            checks: Vec::new(),
            ran_at_unix_ms: crate::types::current_time_unix_ms(),
        }
    }

    pub(crate) fn push<S: Into<String>>(&mut self, name: &str, passed: bool, detail: S) {
        self.checks.push(CheckResult {
            name: name.to_string(),
            passed,
            detail: detail.into(),
        });
    }

    pub(crate) fn finalize(mut self) -> Self {
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

    /// Render the failed checks as a prompt-ready block. This is what a
    /// Reflexion retry feeds back to the agent, so it lists only the
    /// failures, verbatim, with per-check detail capped so one noisy
    /// cargo error can't blow the prompt budget.
    pub fn failure_block(&self) -> String {
        const MAX_DETAIL_CHARS: usize = 2000;
        let mut out = format!("validator `{}` rejected: {}\n", self.stage, self.headline);
        for c in self.checks.iter().filter(|c| !c.passed) {
            let detail: String = if c.detail.chars().count() > MAX_DETAIL_CHARS {
                let truncated: String = c.detail.chars().take(MAX_DETAIL_CHARS).collect();
                format!("{truncated}… (truncated)")
            } else {
                c.detail.clone()
            };
            out.push_str(&format!("- FAILED {}: {}\n", c.name, detail));
        }
        out
    }

    /// Render the full checklist for terminal output (`evolvo_desktop
    /// validate`). One line per check, pass/fail marker first so an agent
    /// can grep `^FAIL`.
    pub fn render_text(&self) -> String {
        let mut out = String::new();
        for c in &self.checks {
            let marker = if c.passed { "ok  " } else { "FAIL" };
            out.push_str(&format!("{marker}  {} — {}\n", c.name, c.detail));
        }
        out.push_str(&format!(
            "\n{}: {}\n",
            if self.passed { "PASSED" } else { "REJECTED" },
            self.headline
        ));
        out
    }
}

/// Build a single-check failed report outside the normal validators —
/// used when the stage can't even be validated (plan.json missing or
/// unparseable after the agent ran), so the failure still flows through
/// the same StageReport machinery as a real rejection.
pub fn report_failure(stage: &str, check_name: &str, detail: &str) -> StageReport {
    let mut r = StageReport::new(stage);
    r.push(check_name, false, detail);
    r.finalize()
}

// --- Planner validators (read-only: inspect `plan.json` structure). ---

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

    validate_behavior_plan(&mut r, plan);

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

/// Minimum length for a `staticApp.reason` — a real sentence, not "n/a".
const MIN_STATIC_REASON_CHARS: usize = 20;

/// Behaviour-first planning checks: a plan must say what the app *does*
/// (or explicitly justify being static), every behaviour must be concrete
/// enough to hold the implementer to, and AI-shaped requests must run a
/// real model instead of shipping bookkeeping records.
fn validate_behavior_plan(r: &mut StageReport, plan: &IterationPlan) {
    use crate::plan::{BEHAVIOR_ENGINES, BEHAVIOR_TRIGGERS, LLM_TEMPLATES};
    let behaviors = &plan.backend.behaviors;
    let static_reason = plan
        .backend
        .static_app
        .as_ref()
        .map(|s| s.reason.trim().to_string());
    let static_ok = static_reason
        .as_ref()
        .is_some_and(|reason| reason.chars().count() >= MIN_STATIC_REASON_CHARS);
    r.push(
        "plan declares live behaviors (or justifies a static app)",
        !behaviors.is_empty() || static_ok,
        if !behaviors.is_empty() {
            format!("{} behaviors", behaviors.len())
        } else if static_reason.is_some() {
            format!(
                "backend.staticApp.reason must explain why nothing on screen changes on its own (>= {MIN_STATIC_REASON_CHARS} chars)"
            )
        } else {
            "backend.behaviors is empty — every drawn element showing changing state \
             (statuses, \"running\", counts, timestamps, threads, replies, progress) needs a \
             behavior that really produces that change; set backend.staticApp only if the app \
             truly has none"
                .to_string()
        },
    );

    let command_names: std::collections::HashSet<&str> = plan
        .backend
        .commands
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    let mut incomplete: Vec<String> = Vec::new();
    for b in behaviors {
        let mut problems: Vec<&str> = Vec::new();
        if !is_snake_case(&b.name) {
            problems.push("name not snake_case");
        }
        if !BEHAVIOR_TRIGGERS.contains(&b.trigger.as_str()) {
            problems.push("trigger not one of userAction|timer|backgroundWorker|externalEvent");
        }
        if b.trigger == "userAction" && !command_names.contains(b.triggered_by.trim()) {
            problems.push("userAction triggeredBy must name a planned command");
        }
        if b.trigger != "userAction" && b.triggered_by.trim().is_empty() {
            problems.push("triggeredBy empty");
        }
        if !BEHAVIOR_ENGINES.contains(&b.engine.as_str()) {
            problems.push("engine not deterministic|llm");
        }
        if b.effect.trim().is_empty() {
            problems.push("effect empty");
        }
        if b.changes_state.is_empty() {
            problems.push("changesState empty");
        }
        if !is_snake_case(&b.implemented_in) {
            problems.push("implementedIn must be a snake_case fn name");
        }
        if b.body_sketch.trim().len() < 30 {
            problems.push("bodySketch < 30 chars");
        }
        if anchors_or_sketch(&b.anchors, &b.body_sketch).is_empty() {
            problems.push("no anchors");
        }
        if !problems.is_empty() {
            incomplete.push(format!("{}: {}", b.name, problems.join(", ")));
        }
    }
    r.push(
        "every behavior is concrete (trigger, effect, state, engine, implementedIn, sketch)",
        incomplete.is_empty(),
        if incomplete.is_empty() {
            "ok".to_string()
        } else {
            incomplete.join(" | ")
        },
    );

    let llm_behaviors = behaviors.iter().filter(|b| b.engine == "llm").count();
    let has_llm_template = plan
        .templates
        .use_templates
        .iter()
        .any(|t| LLM_TEMPLATES.contains(&t.as_str()));
    if llm_behaviors > 0 {
        r.push(
            "llm behaviors pull in an LLM integration template",
            has_llm_template,
            if has_llm_template {
                "ok".to_string()
            } else {
                format!(
                    "{llm_behaviors} llm behavior(s) but templates.useTemplates has none of {LLM_TEMPLATES:?} \
                     — prefer claude_cli (local CLI, no API key)"
                )
            },
        );
    }

    let signals = crate::plan::ai_signals(plan);
    if !signals.is_empty() && plan.backend.static_app.is_none() {
        r.push(
            "AI-shaped request runs a real model (>= 1 llm behavior)",
            llm_behaviors > 0,
            if llm_behaviors > 0 {
                format!("{llm_behaviors} llm behavior(s); signals {signals:?}")
            } else {
                format!(
                    "the request mentions {signals:?} — agents/assistants must actually do their \
                     work through a model (engine \"llm\" + claude_cli template), not sit as \
                     bookkeeping records. If the user really wants a non-AI app, set \
                     backend.staticApp with the reason (the job will pause for review)."
                )
            },
        );
    }
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

    validate_design_plan(&mut r, plan);
    r.finalize()
}

/// Vocabulary for `DesignPlan.layout_pattern`. Kept narrow deliberately —
/// these are the shapes `_design_guidelines.md` and the review UI both
/// know how to render; a planner that needs something else should still
/// pick the closest fit rather than inventing a new string the UI can't
/// badge.
const LAYOUT_PATTERNS: &[&str] = &["stack", "sidebar", "grid", "dashboard", "split"];

/// Characters that would let a `theme_overrides[].value` break out of a
/// CSS custom-property declaration or inject markup if ever rendered
/// unescaped. This is a value that lands in `:root { --token: <value>; }`
/// in app.css and is also rendered as plain text in the review UI, so both
/// CSS-syntax and HTML-syntax injection are in scope.
const INJECTION_CHARS: &[char] = &[';', '{', '}', '<'];

fn validate_design_plan(r: &mut StageReport, plan: &IterationPlan) {
    let design = &plan.frontend.design;

    if !design.layout_pattern.is_empty() {
        r.push(
            "design.layoutPattern is a known pattern",
            LAYOUT_PATTERNS.contains(&design.layout_pattern.as_str()),
            format!(
                "`{}` — must be one of {:?} (empty is allowed for legacy plans)",
                design.layout_pattern, LAYOUT_PATTERNS
            ),
        );
    }

    let mut bad_tokens: Vec<String> = Vec::new();
    let mut bad_values: Vec<String> = Vec::new();
    for ov in &design.theme_overrides {
        if !REQUIRED_TOKEN_NAMES.contains(&ov.token.as_str()) {
            bad_tokens.push(ov.token.clone());
        }
        let value_ok = !ov.value.trim().is_empty()
            && !ov.value.contains(INJECTION_CHARS)
            && !ov.value.contains('\n')
            && !ov.value.contains('\r');
        if !value_ok {
            bad_values.push(format!("{}: `{}`", ov.token, ov.value));
        }
    }
    r.push(
        "design.themeOverrides reference known token names",
        bad_tokens.is_empty(),
        if bad_tokens.is_empty() {
            "ok".to_string()
        } else {
            format!(
                "not in REQUIRED_TOKEN_NAMES: {:?} — theme_overrides may only re-value tokens \
                 that already exist in tokens.css",
                bad_tokens
            )
        },
    );
    r.push(
        "design.themeOverrides values are safe + non-empty",
        bad_values.is_empty(),
        if bad_values.is_empty() {
            "ok".to_string()
        } else {
            format!(
                "invalid value(s): {:?} — must be non-empty and must not contain ';', '{{', '}}', '<', or a newline",
                bad_values
            )
        },
    );

    let mut bad_notes: Vec<String> = Vec::new();
    for note in &design.visual_notes {
        if note.region.trim().is_empty() || note.interpretation.trim().is_empty() {
            bad_notes.push(format!(
                "region=`{}` interpretation=`{}`",
                note.region, note.interpretation
            ));
        }
    }
    r.push(
        "design.visualNotes have non-empty region + interpretation",
        bad_notes.is_empty(),
        if bad_notes.is_empty() {
            "ok".to_string()
        } else {
            format!("incomplete note(s): {:?}", bad_notes)
        },
    );
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

    let behavior_names: std::collections::HashSet<&str> = plan
        .backend
        .behaviors
        .iter()
        .map(|b| b.name.as_str())
        .collect();
    let verified: std::collections::HashSet<&str> = plan
        .e2e
        .scenarios
        .iter()
        .flat_map(|s| s.verifies_behaviors.iter().map(String::as_str))
        .collect();
    let uncovered: Vec<&str> = plan
        .backend
        .behaviors
        .iter()
        .map(|b| b.name.as_str())
        .filter(|name| !verified.contains(name))
        .collect();
    let unknown: Vec<&str> = verified
        .iter()
        .copied()
        .filter(|name| !behavior_names.contains(name))
        .collect();
    r.push(
        "every behavior has a scenario asserting its effect",
        uncovered.is_empty() && unknown.is_empty(),
        if uncovered.is_empty() && unknown.is_empty() {
            format!("{} behaviors covered", behavior_names.len())
        } else {
            format!(
                "uncovered behaviors: {uncovered:?}; unknown names in verifiesBehaviors: {unknown:?} \
                 — add scenarios whose steps assert the effect the user did NOT type \
                 (e.g. 'Expect a reply from Coder to appear in the message bus')"
            )
        },
    );
    r.finalize()
}

// --- Impl validators (inspect worktree files + run cargo). ---

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

    push_behavior_impl_checks(&mut r, plan, &host_src_body, &invoke_body);

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
    push_tool_gate(
        &mut r,
        "cargo check -p evolvo_desktop",
        run_cargo(worktree, &["check", "-p", "evolvo_desktop"]),
    );
    push_tool_gate(
        &mut r,
        "cargo test -p evolvo_desktop --lib",
        run_cargo(worktree, &["test", "-p", "evolvo_desktop", "--lib"]),
    );
    r.finalize()
}

/// Minimum bytes of new CSS the FrontendImpl stage must add on top of the
/// snapshot taken at pipeline start. 50 bytes is a loose floor — it rejects
/// "touched the file but added nothing" and "added one empty selector" while
/// leaving room for legitimately-small redesigns that mostly reuse classes.
pub const STYLES_MIN_GROWTH_BYTES: u64 = 50;

/// Token names that must be defined in `app/ui/styles/tokens.css`. Values
/// may change per-iteration (that's the creative dial); a missing NAME is a
/// FAIL — code across the layered stylesheets (base.css, app.css) assumes
/// these exist. Keep in lockstep with `app/ui/styles/tokens.css`.
pub const REQUIRED_TOKEN_NAMES: &[&str] = &[
    "--bg",
    "--fg",
    "--muted",
    "--border",
    "--panel-bg",
    "--panel-shadow",
    "--accent",
    "--accent-fg",
    "--accent-hover",
    "--danger",
    "--success",
    "--radius",
    "--tool-size",
    "--space-1",
    "--space-2",
    "--space-3",
    "--space-4",
    "--space-5",
    "--space-6",
    "--space-7",
    "--space-8",
    "--text-xs",
    "--text-sm",
    "--text-base",
    "--text-lg",
    "--text-xl",
    "--text-2xl",
    "--weight-normal",
    "--weight-medium",
    "--weight-semibold",
    "--weight-bold",
    "--surface",
    "--surface-raised",
    "--surface-sunken",
    "--fg-strong",
    "--shadow-1",
    "--shadow-2",
    "--shadow-3",
    "--transition-fast",
    "--transition-base",
    "--radius-sm",
    "--radius-full",
];

/// Strip `/* ... */` CSS comments before scanning for lint violations, so a
/// hex color or raw value mentioned only in prose (e.g. "// was #fff
/// before") doesn't false-positive. Not a full CSS parser — good enough for
/// grep-shaped checks over well-formed stylesheets.
///
/// Byte-level scanning is safe on UTF-8 here: `/`, `*`, and `\n` are all
/// single-byte ASCII code points, and no multi-byte UTF-8 continuation byte
/// can equal one of them, so slicing on their byte offsets never lands
/// inside a multi-byte character.
fn strip_css_comments(body: &str) -> String {
    let bytes = body.as_bytes();
    let mut out = String::with_capacity(body.len());
    let mut in_comment = false;
    let mut i = 0;
    while i < bytes.len() {
        if !in_comment && bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
            in_comment = true;
            i += 2;
            continue;
        }
        if in_comment {
            if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                in_comment = false;
                i += 2;
                continue;
            }
            if bytes[i] == b'\n' {
                out.push('\n'); // keep line numbers aligned
            }
            i += 1;
            continue;
        }
        let ch_len = utf8_char_len(bytes[i]);
        out.push_str(&body[i..i + ch_len]);
        i += ch_len;
    }
    out
}

/// Length in bytes of the UTF-8 character starting with `lead_byte`.
fn utf8_char_len(lead_byte: u8) -> usize {
    if lead_byte & 0x80 == 0 {
        1
    } else if lead_byte & 0xE0 == 0xC0 {
        2
    } else if lead_byte & 0xF0 == 0xE0 {
        3
    } else {
        4
    }
}

/// One lint finding: 1-based line number + human-readable detail.
struct LintFinding {
    line: usize,
    detail: String,
}

/// True when a (comment-stripped) line is a CSS custom-property
/// **definition** — i.e. matches `--<name>: <value>;` in declaration
/// position. Defining a token's value is exactly where a raw color
/// belongs (`--accent: #7c3aed;` inside a `:root { ... }` block is the
/// point of `theme_overrides`), so these lines are exempt from the
/// raw-hex/rgb/hsl lints. A *use* of a property (`color: var(--accent)`)
/// does not start with `--` and is still fully linted.
///
/// Deliberately conservative: only the declaration's left-hand side is
/// checked (trimmed line starts with `--` and contains a `:`), so this
/// cannot be used to smuggle raw colors into a normal property value by
/// prefixing something unrelated with `--`.
fn is_custom_property_declaration(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("--") && trimmed.contains(':')
}

/// Token lint over an iteration-owned stylesheet body (comments already
/// stripped). Scans for `#RGB`/`#RGBA`/`#RRGGBB`/`#RRGGBBAA` value-position
/// hex literals. No regex crate in this workspace's dependency set for host
/// code, so this is a small manual scanner rather than pulling in a new
/// dependency for one lint. Returns every violation found — callers turn an
/// empty vec into a passing check.
fn lint_hex_colors(body: &str) -> Vec<LintFinding> {
    let mut out = Vec::new();
    for (line_idx, line) in body.lines().enumerate() {
        if is_custom_property_declaration(line) {
            continue;
        }
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'#' {
                let start = i;
                let mut j = i + 1;
                while j < bytes.len() && bytes[j].is_ascii_hexdigit() {
                    j += 1;
                }
                let hex_len = j - start - 1;
                if matches!(hex_len, 3 | 4 | 6 | 8) {
                    out.push(LintFinding {
                        line: line_idx + 1,
                        detail: format!(
                            "hex color literal `{}` — use var(--...) instead",
                            &line[start..j]
                        ),
                    });
                }
                i = j;
            } else {
                i += 1;
            }
        }
    }
    out
}

fn lint_color_functions(body: &str) -> Vec<LintFinding> {
    let mut out = Vec::new();
    for (line_idx, line) in body.lines().enumerate() {
        if is_custom_property_declaration(line) {
            continue;
        }
        for needle in ["rgb(", "rgba(", "hsl(", "hsla("] {
            if line.contains(needle) {
                out.push(LintFinding {
                    line: line_idx + 1,
                    detail: format!("`{needle}` literal — use var(--...) instead"),
                });
            }
        }
    }
    out
}

fn lint_font_size_declarations(body: &str) -> Vec<LintFinding> {
    let mut out = Vec::new();
    for (line_idx, line) in body.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("font-size:") && !line.contains("var(--text-") {
            out.push(LintFinding {
                line: line_idx + 1,
                detail: format!("`{}` must reference var(--text-*)", trimmed.trim()),
            });
        }
    }
    out
}

fn lint_box_shadow_declarations(body: &str) -> Vec<LintFinding> {
    let mut out = Vec::new();
    for (line_idx, line) in body.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("box-shadow:")
            && !line.contains("var(--shadow-")
            && !trimmed.contains("none")
        {
            out.push(LintFinding {
                line: line_idx + 1,
                detail: format!(
                    "`{}` must reference var(--shadow-*) (or be `none`)",
                    trimmed.trim()
                ),
            });
        }
    }
    out
}

fn push_lint_check(r: &mut StageReport, name: &str, findings: &[LintFinding]) {
    if findings.is_empty() {
        r.push(name, true, "ok");
        return;
    }
    let detail = findings
        .iter()
        .take(10)
        .map(|f| format!("line {}: {}", f.line, f.detail))
        .collect::<Vec<_>>()
        .join("; ");
    let suffix = if findings.len() > 10 {
        format!(" (+{} more)", findings.len() - 10)
    } else {
        String::new()
    };
    r.push(name, false, format!("{detail}{suffix}"));
}

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

    // Iteration-owned stylesheet delta gate — the FrontendImpl stage must
    // *add* CSS for the NewApp's new components / routes. A previous
    // iteration shipped a behaviourally-correct app with zero styling
    // because the model never touched the stylesheet. The snapshot taken
    // at pipeline start lives at `<job_dir>/inputs/styles.baseline.css`;
    // we require the current file to exist and to have grown by at least
    // `STYLES_MIN_GROWTH_BYTES`.
    //
    // BACKWARD COMPAT: `app/ui/styles/app.css` is the growth target when
    // present (current layered layout). A worktree cloned from before the
    // tokens/base/app split has no `styles/app.css` at all — in that case
    // `iteration_styles_path` falls back to the legacy monolithic
    // `app/ui/styles.css`, and the snapshot taken at pipeline start will
    // have targeted the same fallback (see `stages::snapshot_styles_baseline`),
    // so the delta comparison stays apples-to-apples either way.
    let styles_path = crate::stages::iteration_styles_path(worktree);
    let styles_rel_display = styles_path
        .strip_prefix(worktree)
        .unwrap_or(&styles_path)
        .display()
        .to_string();
    let baseline_path = job_dir
        .join("inputs")
        .join(crate::stages::STYLES_BASELINE_FILENAME);
    let styles_len = std::fs::metadata(&styles_path).map(|m| m.len()).ok();
    let baseline_len = std::fs::metadata(&baseline_path).map(|m| m.len()).ok();
    r.push(
        &format!("{styles_rel_display} exists"),
        styles_len.is_some(),
        styles_path.display().to_string(),
    );
    match (styles_len, baseline_len) {
        (Some(now), Some(base)) => {
            let grew_by = now.saturating_sub(base);
            r.push(
                &format!("{styles_rel_display} grew vs baseline"),
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
                &format!("{styles_rel_display} grew vs baseline"),
                true,
                "no baseline snapshot — skipped",
            );
        }
        _ => { /* first check already reported the missing file */ }
    }

    // Token lint — only runs against the layered `styles/app.css` file
    // (legacy worktrees with no split have no tokens.css to lint against
    // either, so they're exempt from here on down).
    let app_css_path = worktree.join("app/ui/styles/app.css");
    if app_css_path.exists() {
        let raw = read(&app_css_path);
        let body = strip_css_comments(&raw);

        push_lint_check(
            &mut r,
            "styles/app.css: no hex color literals",
            &lint_hex_colors(&body),
        );
        push_lint_check(
            &mut r,
            "styles/app.css: no rgb()/rgba()/hsl() literals",
            &lint_color_functions(&body),
        );
        push_lint_check(
            &mut r,
            "styles/app.css: font-size uses var(--text-*)",
            &lint_font_size_declarations(&body),
        );
        push_lint_check(
            &mut r,
            "styles/app.css: box-shadow uses var(--shadow-*)",
            &lint_box_shadow_declarations(&body),
        );
        r.push(
            "styles/app.css: has a :hover rule",
            body.contains(":hover"),
            "at least one interactive :hover state is required",
        );
        r.push(
            "styles/app.css: has a :focus-visible rule",
            body.contains(":focus-visible"),
            "at least one interactive :focus-visible state is required",
        );

        // Token integrity gate — tokens.css must exist and define every
        // required token NAME (values are free to change per-iteration).
        let tokens_path = worktree.join("app/ui/styles/tokens.css");
        let tokens_body = read(&tokens_path);
        r.push(
            "styles/tokens.css exists",
            tokens_path.exists(),
            tokens_path.display().to_string(),
        );
        let missing_tokens: Vec<&str> = REQUIRED_TOKEN_NAMES
            .iter()
            .filter(|name| !tokens_body.contains(*name))
            .copied()
            .collect();
        r.push(
            "styles/tokens.css defines every required token",
            missing_tokens.is_empty(),
            if missing_tokens.is_empty() {
                "ok".to_string()
            } else {
                format!("missing: {}", missing_tokens.join(", "))
            },
        );

        // Design-plan coherence: every token the plan promised to override
        // must actually be re-declared in app.css (the `:root { ... }`
        // block the implementer is instructed to write at the top of the
        // file). We do NOT require the reverse — hand-tuned overrides with
        // no plan entry are fine, the plan is a floor on intent-to-CSS
        // traceability, not a ceiling on creativity.
        let unapplied: Vec<&str> = plan
            .frontend
            .design
            .theme_overrides
            .iter()
            .filter(|ov| !body.contains(&format!("{}:", ov.token)))
            .map(|ov| ov.token.as_str())
            .collect();
        if !plan.frontend.design.theme_overrides.is_empty() {
            r.push(
                "design.themeOverrides applied in styles/app.css",
                unapplied.is_empty(),
                if unapplied.is_empty() {
                    "ok".to_string()
                } else {
                    format!(
                        "planned theme override(s) not applied in app.css: {}",
                        unapplied.join(", ")
                    )
                },
            );
        }
    }

    push_tool_gate(
        &mut r,
        "cargo check -p evolvo_ui (wasm)",
        run_cargo(
            worktree,
            &[
                "check",
                "-p",
                "evolvo_ui",
                "--target",
                "wasm32-unknown-unknown",
            ],
        ),
    );
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
        r.push(
            "CLAUDE.md rewritten for new app (iteration 1)",
            passed,
            detail,
        );
    }

    // Product-invariant grep gates (I-P1..I-P3b). The NewApp content area
    // (`app.rs`) may be rewritten freely, but the invariant chrome must
    // survive every iteration: shell.rs keeps the single Feedback FAB and
    // its one `panel_open` signal, app.rs keeps mounting <Shell>, and the
    // overlay CSS keeps `.canvas-overlay`. Previously these were enforced
    // only by prompt prose — and regressed more than once. Only checkable
    // on the stock stack; a rewritten stack is reviewed by hand.
    let shell_rs_path = worktree.join("app/ui/src/shell.rs");
    if shell_rs_path.exists() {
        let shell_body = read(&shell_rs_path);
        r.push(
            "shell.rs keeps FeedbackFab + panel_open (I-P3b)",
            shell_body.contains("FeedbackFab") && shell_body.contains("panel_open"),
            if shell_body.contains("FeedbackFab") {
                "panel_open signal missing from shell.rs".to_string()
            } else {
                "FeedbackFab missing from shell.rs — the single feedback trigger is invariant chrome".to_string()
            },
        );
        let app_body = read(&worktree.join("app/ui/src/app.rs"));
        r.push(
            "app.rs still mounts <Shell> (I-P1/I-P2/I-P3)",
            app_body.contains("Shell"),
            "NewApp content must render inside <Shell> so Lineage + Feedback overlay stay reachable",
        );
        let styles_body = read(&worktree.join("app/ui/styles.css"));
        r.push(
            ".canvas-overlay styles present (I-P3)",
            styles_body.contains(".canvas-overlay"),
            "canvas overlay CSS removed — the per-page annotation overlay would not render",
        );

        // Design token vocabulary is protected chrome too: base.css and
        // app.css both assume every token name in REQUIRED_TOKEN_NAMES
        // resolves. Only enforced when the layered stylesheet layout is
        // present — a worktree that predates the split (or a stack that
        // deliberately rewrote the whole styling story) is out of scope.
        let tokens_path = worktree.join("app/ui/styles/tokens.css");
        if tokens_path.exists() {
            let tokens_body = read(&tokens_path);
            let missing_tokens: Vec<&str> = REQUIRED_TOKEN_NAMES
                .iter()
                .filter(|name| !tokens_body.contains(*name))
                .copied()
                .collect();
            r.push(
                "styles/tokens.css defines every required token",
                missing_tokens.is_empty(),
                if missing_tokens.is_empty() {
                    "ok".to_string()
                } else {
                    format!("missing: {}", missing_tokens.join(", "))
                },
            );
        }
    } else {
        r.push(
            "shell invariants (I-P1..I-P3b)",
            true,
            "no app/ui/src/shell.rs — stack rewritten; invariants must be reviewed by hand",
        );
    }

    // The frontend must actually BUILD, not just type-check. `cargo check`
    // passes while Trunk fails on index.html asset errors, hook scripts, or
    // wasm-bindgen version drift — and a Trunk failure means the reviewer's
    // Run lands on a blank WebView. Gate on `trunk build` whenever the
    // standard UI stack is present; a rewritten stack (no Trunk.toml) is
    // covered by the e2e run-iteration.sh check instead.
    let ui_dir = worktree.join("app/ui");
    if ui_dir.join("Trunk.toml").exists() {
        push_tool_gate(
            &mut r,
            "trunk build (app/ui)",
            run_tool(&ui_dir, "trunk", &["build"]),
        );
    } else {
        r.push(
            "trunk build (app/ui)",
            true,
            "no app/ui/Trunk.toml — stack rewritten, skipped",
        );
    }

    // Design memory: DESIGN.md must exist, be non-trivial, and retain the
    // decision-log section header so the next iteration's frontend stages
    // have somewhere to read continuity from and somewhere to append to.
    // A missing/gutted DESIGN.md silently resets design memory to zero
    // every iteration, which defeats the entire point of Part C.
    // Mockup gate: every planned behaviour must still exist and be wired
    // after all later stages (frontend / e2e work can orphan a worker).
    if !plan.backend.behaviors.is_empty() {
        let host_src_body = collect_host_rs_files(worktree)
            .iter()
            .map(|p| read(p))
            .collect::<Vec<_>>()
            .join("\n");
        let invoke_body = format!(
            "{}\n{}",
            read(&worktree.join("app/src-tauri/src/lib.rs")),
            read(&worktree.join("app/src-tauri/src/main.rs"))
        );
        push_behavior_impl_checks(&mut r, plan, &host_src_body, &invoke_body);
    }

    let (design_ok, design_detail) = check_design_md(&worktree.join("DESIGN.md"));
    r.push(
        "DESIGN.md exists and has a decision log",
        design_ok,
        design_detail,
    );

    r.finalize()
}

/// Minimum size (bytes) for `DESIGN.md` to count as "non-trivial" — high
/// enough to reject a one-line stub, low enough not to force padding.
const DESIGN_MD_MIN_BYTES: u64 = 500;

fn check_design_md(path: &Path) -> (bool, String) {
    let body = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return (false, "DESIGN.md missing at worktree root".to_string()),
    };
    let len = body.len() as u64;
    if len < DESIGN_MD_MIN_BYTES {
        return (
            false,
            format!("DESIGN.md is only {len}B (min {DESIGN_MD_MIN_BYTES}B) — looks like a stub"),
        );
    }
    if !body.contains("## Design decisions") {
        return (
            false,
            "DESIGN.md is missing the `## Design decisions` log section".to_string(),
        );
    }
    (true, "ok".to_string())
}

/// Build gates for **single-session (legacy) lineage runs** — every
/// non-NewApp feedback. Exit code 0 from the agent CLI means "the agent
/// process didn't crash", not "the change is good"; the runner calls this
/// before flipping such a job to `BuildReady`. The checks are the repo's
/// own commit contract, made tolerant of rewritten stacks: no root
/// Cargo.toml → gates are skipped with an explicit note rather than
/// failing a stack we can't recognise.
pub fn validate_build_gates(worktree: &Path) -> StageReport {
    let mut r = StageReport::new("build_gates");
    if !worktree.join("Cargo.toml").exists() {
        r.push(
            "cargo workspace present",
            true,
            "no root Cargo.toml — stack rewritten; build gates skipped",
        );
        return r.finalize();
    }
    push_tool_gate(
        &mut r,
        "cargo check --workspace",
        run_cargo(worktree, &["check", "--workspace"]),
    );
    push_tool_gate(
        &mut r,
        "cargo test --workspace --lib",
        run_cargo(worktree, &["test", "--workspace", "--lib"]),
    );
    // The wasm target only applies while the stock UI crate is present —
    // a renamed/rewritten UI crate is allowed (freedom levels permit it).
    let ui_manifest = read(&worktree.join("app/ui/Cargo.toml"));
    if ui_manifest.contains("name = \"evolvo_ui\"") {
        push_tool_gate(
            &mut r,
            "cargo check -p evolvo_ui (wasm)",
            run_cargo(
                worktree,
                &[
                    "check",
                    "-p",
                    "evolvo_ui",
                    "--target",
                    "wasm32-unknown-unknown",
                ],
            ),
        );
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
    anchors_or_sketch(&cmd.anchors, &cmd.body_sketch)
}

fn anchors_or_sketch(anchors: &[String], body_sketch: &str) -> Vec<String> {
    if !anchors.is_empty() {
        return anchors
            .iter()
            .map(|a| a.trim().to_string())
            .filter(|a| !a.is_empty())
            .collect();
    }
    extract_identifier_anchors(body_sketch)
}

const ANCHOR_STOPWORDS: &[&str] = &[
    "and", "the", "for", "into", "from", "with", "return", "returns", "self", "Self", "store",
    "result", "ok", "err", "true", "false", "let", "mut", "fn", "pub", "use", "ref", "match", "if",
    "else", "loop", "while", "as", "in", "of", "to", "on", "this", "that", "then", "build", "make",
    "call", "id", "rec", "row", "row_id", "value", "string", "vec", "option",
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
/// Count references to identifier `name` in `src` that are not its own
/// `fn name` definition — i.e. call sites, registrations, spawns.
fn count_non_definition_refs(src: &str, name: &str) -> usize {
    let bytes = src.as_bytes();
    let is_ident = |b: u8| b == b'_' || b.is_ascii_alphanumeric();
    let mut count = 0;
    let mut start = 0;
    while let Some(rel) = src[start..].find(name) {
        let pos = start + rel;
        let end = pos + name.len();
        let before_ok = pos == 0 || !is_ident(bytes[pos - 1]);
        let after_ok = end >= bytes.len() || !is_ident(bytes[end]);
        if before_ok && after_ok {
            let is_def = src[..pos].trim_end().ends_with("fn");
            if !is_def {
                count += 1;
            }
        }
        start = end;
    }
    count
}

/// Behaviour implementation checks, shared by `backend_impl` and the final
/// review: each planned behaviour's function exists, its body contains the
/// planned anchors, something actually invokes it, and llm behaviours have
/// an LLM integration in the crate.
fn push_behavior_impl_checks(
    r: &mut StageReport,
    plan: &IterationPlan,
    host_src_body: &str,
    invoke_body: &str,
) {
    for b in &plan.backend.behaviors {
        let fn_name = b.implemented_in.trim();
        let body = extract_function_body(host_src_body, fn_name);
        r.push(
            &format!("behavior `{}` implemented in `fn {}`", b.name, fn_name),
            body.is_some(),
            "search every .rs under app/src-tauri/src",
        );
        let Some(body) = body else { continue };
        let impl_body =
            extract_function_body(host_src_body, &format!("{fn_name}_impl")).unwrap_or_default();
        let combined = format!("{body}\n{impl_body}");
        let anchors = anchors_or_sketch(&b.anchors, &b.body_sketch);
        let hit = anchors.is_empty() || anchors.iter().any(|a| combined.contains(a.as_str()));
        r.push(
            &format!("behavior `{}` body references an anchor", b.name),
            hit,
            if hit {
                "ok".to_string()
            } else {
                format!("`{fn_name}` mentions none of {anchors:?} — possible simulated behavior")
            },
        );
        let registered = invoke_body.contains(&format!("commands::{fn_name}"));
        let reachable = registered || count_non_definition_refs(host_src_body, fn_name) > 0;
        r.push(
            &format!("behavior `{}` is reachable", b.name),
            reachable,
            if reachable {
                "ok".to_string()
            } else {
                format!(
                    "nothing calls `{fn_name}` — wire it to its trigger ({} via {})",
                    b.trigger, b.triggered_by
                )
            },
        );
        if b.engine == "llm" {
            let wired = crate::plan::LLM_TEMPLATES
                .iter()
                .any(|t| host_src_body.contains(t));
            r.push(
                &format!("llm behavior `{}` has an LLM integration in the crate", b.name),
                wired,
                "copy templates/integrations/claude_cli (or openrouter) into app/src-tauri/src/integrations/",
            );
        }
    }
}

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
    // `// FILL:` is the marker the prompts use for scaffold holes the
    // implementer must replace — one surviving into a "done" stage means
    // the work wasn't finished.
    if body.contains("// FILL") || body.contains("FILL:") {
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
            let rest = &body.as_bytes()[pos..];
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

/// Hard ceiling for one build-tool invocation inside a validator. A
/// cold-worktree `cargo check` of the Tauri stack takes single-digit
/// minutes; 15 gives slow machines headroom while still guaranteeing a
/// hung rustc / wasm-bindgen can't wedge the stage forever.
const TOOL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// Outcome of one build-tool run: exit success plus the tail of its
/// combined stdout/stderr, so a failed check can say *what* broke instead
/// of "exited non-zero" — that tail is exactly what the Reflexion retry
/// prompt needs to fix the right thing.
pub(crate) struct ToolRun {
    pub(crate) success: bool,
    pub(crate) tail: String,
}

fn run_cargo(worktree: &Path, args: &[&str]) -> Result<ToolRun, String> {
    run_tool(worktree, "cargo", args)
}

/// Run a build tool to completion with the process-group timeout from
/// `proc.rs` and the GUI-safe PATH from the runner (a packaged app
/// launched from Finder inherits a PATH without ~/.cargo/bin, which is
/// where cargo and trunk actually live). Output is captured via a temp
/// file — piping would deadlock once the child writes more than the pipe
/// buffer, which cargo error output easily does.
pub(crate) fn run_tool(dir: &Path, program: &str, args: &[&str]) -> Result<ToolRun, String> {
    run_tool_with_timeout(dir, program, args, TOOL_TIMEOUT)
}

/// `run_tool` with an explicit timeout — external-app gates (see
/// `external.rs`) build arbitrary, often much larger, projects.
pub(crate) fn run_tool_with_timeout(
    dir: &Path,
    program: &str,
    args: &[&str],
    timeout: std::time::Duration,
) -> Result<ToolRun, String> {
    let capture = tempfile::NamedTempFile::new().map_err(|e| format!("tool capture file: {e}"))?;
    let out = capture
        .reopen()
        .map_err(|e| format!("reopen capture file: {e}"))?;
    let err = out
        .try_clone()
        .map_err(|e| format!("clone capture handle: {e}"))?;

    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(dir)
        .env("PATH", crate::runner::enriched_path())
        .stdout(std::process::Stdio::from(out))
        .stderr(std::process::Stdio::from(err));
    match crate::proc::run_with_timeout(&mut cmd, timeout) {
        Ok(crate::proc::WaitOutcome::Exited(s)) => Ok(ToolRun {
            success: s.success(),
            tail: read_tail(capture.path(), 1500),
        }),
        Ok(crate::proc::WaitOutcome::TimedOut) => Err(format!(
            "{program} timed out after {}s (process group killed)",
            timeout.as_secs()
        )),
        Err(e) => Err(format!("spawn {program}: {e}")),
    }
}

/// Last `max_chars` of a file, lossily decoded. Best-effort — an
/// unreadable capture file yields an empty tail, never an error.
fn read_tail(path: &Path, max_chars: usize) -> String {
    let Ok(bytes) = std::fs::read(path) else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&bytes);
    let text = text.trim_end();
    let chars: Vec<char> = text.chars().collect();
    let start = chars.len().saturating_sub(max_chars);
    chars[start..].iter().collect()
}

/// Push one tool-gate result onto the report, carrying the output tail
/// into the detail on failure.
pub(crate) fn push_tool_gate(r: &mut StageReport, name: &str, res: Result<ToolRun, String>) {
    match res {
        Ok(t) if t.success => r.push(name, true, "ok"),
        Ok(t) => r.push(
            name,
            false,
            if t.tail.is_empty() {
                "exited non-zero (no output captured)".to_string()
            } else {
                format!("exited non-zero — output tail:\n{}", t.tail)
            },
        ),
        Err(e) => r.push(name, false, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{
        AppIdentity, BackendBudget, BackendSection, BehaviorPlan, CanvasReference, CanvasRegion,
        CommandPlan, EntityPlan, FieldPlan, FrontendBudget, FrontendSection, IterationPlan,
        PlanStage, RegionDesignNote, TestPlan, TokenOverride,
    };

    #[test]
    fn failure_block_lists_only_failed_checks_and_caps_detail() {
        // Detail cap is 2000 chars (raised from 500 so a full compiler
        // error tail survives into the Reflexion retry prompt — see
        // `MAX_DETAIL_CHARS`); use a detail comfortably over that so the
        // cap is actually exercised.
        let mut r = StageReport::new("backend_impl");
        r.push("compiles", true, "ok");
        r.push("tests pass", false, "x".repeat(2500));
        r.push("commands registered", false, "missing: create_task");
        let r = r.finalize();

        let block = r.failure_block();
        assert!(block.contains("validator `backend_impl` rejected"));
        assert!(!block.contains("compiles"), "passed checks excluded");
        assert!(block.contains("FAILED tests pass"));
        assert!(block.contains("FAILED commands registered: missing: create_task"));
        assert!(block.contains("(truncated)"), "long detail capped");
        assert!(block.len() < 2500, "block stays prompt-sized");
    }

    #[test]
    fn report_failure_builds_single_check_rejection() {
        let r = report_failure("backend_plan", "plan.json parses", "expected `,` at line 3");
        assert!(!r.passed);
        assert_eq!(r.stage, "backend_plan");
        assert_eq!(r.checks.len(), 1);
        assert!(r.headline.contains("plan.json parses"));
        assert!(r.failure_block().contains("expected `,` at line 3"));
    }

    #[test]
    fn render_text_marks_failures_greppably() {
        let mut r = StageReport::new("e2e_plan");
        r.push("scenarios present", true, "2 scenarios");
        r.push("persistence smoke", false, "missing");
        let text = r.finalize().render_text();
        assert!(text.contains("ok    scenarios present"));
        assert!(text.contains("FAIL  persistence smoke — missing"));
        assert!(text.contains("REJECTED:"));
    }

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
                            "Generate id, build Project, save via store.save_project(rec).".into(),
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
                behaviors: vec![BehaviorPlan {
                    name: "stale_projects_archive".into(),
                    trigger: "timer".into(),
                    triggered_by: "every 60s".into(),
                    effect: "projects untouched for 30 days flip to archived".into(),
                    changes_state: vec!["Project.archived".into()],
                    engine: "deterministic".into(),
                    implemented_in: "archive_stale_projects".into(),
                    body_sketch: "List projects via store.list_projects(), mark stale ones archived, save_project each.".into(),
                    anchors: vec!["save_project".into()],
                    motivated_by_regions: vec!["R1".into()],
                }],
                static_app: None,
                simulated_behaviors: vec![],
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

    fn failed(report: &StageReport, needle: &str) -> bool {
        report
            .checks
            .iter()
            .any(|c| c.name.contains(needle) && !c.passed)
    }

    #[test]
    fn backend_plan_requires_behaviors_or_a_justified_static_app() {
        let mut plan = seed_plan();
        plan.backend.behaviors.clear();
        let report = validate_backend_plan(&plan);
        assert!(failed(&report, "plan declares live behaviors"));

        plan.backend.static_app = Some(crate::plan::StaticAppJustification {
            reason: "n/a".into(),
        });
        assert!(failed(
            &validate_backend_plan(&plan),
            "plan declares live behaviors"
        ));

        plan.backend.static_app = Some(crate::plan::StaticAppJustification {
            reason: "A read-only reference card: nothing on screen changes on its own.".into(),
        });
        let report = validate_backend_plan(&plan);
        assert!(report.passed, "{report:#?}");
    }

    #[test]
    fn backend_plan_rejects_vague_behaviors() {
        let mut plan = seed_plan();
        let b = &mut plan.backend.behaviors[0];
        b.trigger = "whenever".into();
        b.engine = "magic".into();
        b.effect.clear();
        let report = validate_backend_plan(&plan);
        assert!(failed(&report, "every behavior is concrete"));

        let mut plan = seed_plan();
        let b = &mut plan.backend.behaviors[0];
        b.trigger = "userAction".into();
        b.triggered_by = "no_such_command".into();
        assert!(failed(
            &validate_backend_plan(&plan),
            "every behavior is concrete"
        ));
        plan.backend.behaviors[0].triggered_by = "create_project".into();
        assert!(validate_backend_plan(&plan).passed);
    }

    #[test]
    fn ai_shaped_request_must_run_a_real_model() {
        let mut plan = seed_plan();
        plan.user_text = "An agent manager: assign tasks to agents".into();
        let report = validate_backend_plan(&plan);
        assert!(failed(&report, "AI-shaped request runs a real model"));

        // An llm behaviour without an LLM template is still rejected.
        plan.backend.behaviors[0].engine = "llm".into();
        let report = validate_backend_plan(&plan);
        assert!(failed(
            &report,
            "llm behaviors pull in an LLM integration template"
        ));

        plan.templates.use_templates = vec!["claude_cli".into()];
        let report = validate_backend_plan(&plan);
        assert!(report.passed, "{report:#?}");
    }

    #[test]
    fn e2e_plan_must_cover_every_behavior() {
        let mut plan = seed_plan();
        plan.e2e.persistence_smoke = Some(crate::plan::PersistenceSmoke {
            entity: "Project".into(),
            expected_directory: "projects".into(),
        });
        plan.e2e.scenarios = vec![crate::plan::E2EScenario {
            id: "crud".into(),
            title: "crud".into(),
            steps: vec!["Click".into()],
            motivated_by_regions: vec![],
            verifies_behaviors: vec![],
        }];
        assert!(failed(
            &validate_e2e_plan(&plan),
            "every behavior has a scenario"
        ));
        plan.e2e.scenarios[0].verifies_behaviors = vec!["typo_behavior".into()];
        assert!(failed(
            &validate_e2e_plan(&plan),
            "every behavior has a scenario"
        ));
        plan.e2e.scenarios[0].verifies_behaviors = vec!["stale_projects_archive".into()];
        assert!(validate_e2e_plan(&plan).passed);
    }

    #[test]
    fn behavior_impl_checks_existence_anchor_and_reachability() {
        let plan = seed_plan();
        let invoke = "commands::create_project";
        let mut r = StageReport::new("t");
        push_behavior_impl_checks(&mut r, &plan, "fn other() {}", invoke);
        assert!(failed(&r.finalize(), "implemented in"));

        // Defined and anchored but never called → unreachable.
        let src = "fn archive_stale_projects(store: &Store) { store.save_project(&p); }";
        let mut r = StageReport::new("t");
        push_behavior_impl_checks(&mut r, &plan, src, invoke);
        let report = r.finalize();
        assert!(failed(&report, "is reachable"));
        assert!(!failed(&report, "body references an anchor"));

        // Simulated body (no anchor) is flagged even when reachable.
        let src =
            "fn archive_stale_projects() { let _ = 1; }\nfn tick() { archive_stale_projects(); }";
        let mut r = StageReport::new("t");
        push_behavior_impl_checks(&mut r, &plan, src, invoke);
        let report = r.finalize();
        assert!(failed(&report, "body references an anchor"));
        assert!(!failed(&report, "is reachable"));

        let src = "fn archive_stale_projects(s: &Store) { s.save_project(&p); }\n\
                   fn start_timer() { std::thread::spawn(|| archive_stale_projects(&s)); }";
        let mut r = StageReport::new("t");
        push_behavior_impl_checks(&mut r, &plan, src, invoke);
        assert!(r.finalize().passed);
    }

    #[test]
    fn llm_behavior_impl_requires_the_integration_in_the_crate() {
        let mut plan = seed_plan();
        plan.backend.behaviors[0].engine = "llm".into();
        let src = "fn archive_stale_projects(s: &Store) { s.save_project(&p); }\nfn t() { archive_stale_projects(); }";
        let mut r = StageReport::new("t");
        push_behavior_impl_checks(&mut r, &plan, src, "");
        assert!(failed(&r.finalize(), "has an LLM integration"));
        let src = format!("{src}\nmod claude_cli {{}}");
        let mut r = StageReport::new("t");
        push_behavior_impl_checks(&mut r, &plan, &src, "");
        assert!(r.finalize().passed);
    }

    #[test]
    fn non_definition_refs_ignore_the_definition_and_longer_idents() {
        let src = "fn run_turn() {}\nfn run_turn_impl() {}\nfn x() { run_turn(); }";
        assert_eq!(count_non_definition_refs(src, "run_turn"), 1);
        assert_eq!(count_non_definition_refs("fn run_turn() {}", "run_turn"), 0);
    }

    #[test]
    fn backend_plan_validator_rejects_orphan_region() {
        let mut plan = seed_plan();
        plan.backend.entities[0].motivated_by_regions.clear();
        for c in &mut plan.backend.commands {
            c.motivated_by_regions.clear();
        }
        for b in &mut plan.backend.behaviors {
            b.motivated_by_regions.clear();
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

    /// A `seed_plan()` with `frontend.routes`/`components` populated enough
    /// to satisfy the budget + command-reference checks, so design-plan
    /// tests can assert on `report.passed` as a whole without unrelated
    /// budget failures masking the design-specific check they care about.
    fn seed_plan_with_frontend() -> IterationPlan {
        let mut plan = seed_plan();
        plan.frontend.routes.push(crate::plan::RoutePlan {
            path: "/".into(),
            component: "Home".into(),
            uses_commands: vec![],
            motivated_by_regions: vec!["R1".into()],
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
        plan
    }

    #[test]
    fn frontend_plan_validator_accepts_empty_design_plan() {
        // Legacy plans (or a planner that deliberately left design blank)
        // must not be penalised — layoutPattern empty is not a FAIL.
        let plan = seed_plan_with_frontend();
        assert!(plan.frontend.design.layout_pattern.is_empty());
        let report = validate_frontend_plan(&plan);
        assert!(report.passed, "{:?}", report.checks);
    }

    #[test]
    fn frontend_plan_validator_rejects_unknown_layout_pattern() {
        let mut plan = seed_plan_with_frontend();
        plan.frontend.design.layout_pattern = "carousel".into();
        let report = validate_frontend_plan(&plan);
        assert!(!report.passed);
        assert!(report
            .checks
            .iter()
            .any(|c| c.name.contains("layoutPattern") && !c.passed));
    }

    #[test]
    fn frontend_plan_validator_accepts_known_layout_pattern() {
        let mut plan = seed_plan_with_frontend();
        plan.frontend.design.layout_pattern = "sidebar".into();
        let report = validate_frontend_plan(&plan);
        let check = report
            .checks
            .iter()
            .find(|c| c.name.contains("layoutPattern"))
            .expect("layout pattern check present");
        assert!(check.passed, "{:?}", check);
    }

    #[test]
    fn frontend_plan_validator_rejects_theme_override_of_unknown_token() {
        let mut plan = seed_plan_with_frontend();
        plan.frontend.design.theme_overrides.push(TokenOverride {
            token: "--totally-made-up".into(),
            value: "#7c3aed".into(),
            reason: "test".into(),
        });
        let report = validate_frontend_plan(&plan);
        assert!(!report.passed);
        assert!(report
            .checks
            .iter()
            .any(|c| c.name.contains("themeOverrides reference known token") && !c.passed));
    }

    #[test]
    fn frontend_plan_validator_rejects_injection_in_theme_override_value() {
        for bad_value in ["red; } .evil { color", "red<script>", "red{", "red}"] {
            let mut plan = seed_plan_with_frontend();
            plan.frontend.design.theme_overrides.push(TokenOverride {
                token: "--accent".into(),
                value: bad_value.into(),
                reason: "test".into(),
            });
            let report = validate_frontend_plan(&plan);
            assert!(!report.passed, "value {bad_value:?} should be rejected");
        }
    }

    #[test]
    fn frontend_plan_validator_accepts_clean_theme_override() {
        let mut plan = seed_plan_with_frontend();
        plan.frontend.design.theme_overrides.push(TokenOverride {
            token: "--accent".into(),
            value: "#7c3aed".into(),
            reason: "user drew the header purple (R2)".into(),
        });
        let report = validate_frontend_plan(&plan);
        assert!(report.passed, "{:?}", report.checks);
    }

    #[test]
    fn frontend_plan_validator_rejects_incomplete_visual_note() {
        let mut plan = seed_plan_with_frontend();
        plan.frontend.design.visual_notes.push(RegionDesignNote {
            region: "".into(),
            interpretation: "red box = emphasis".into(),
        });
        let report = validate_frontend_plan(&plan);
        assert!(!report.passed);
    }

    #[test]
    fn frontend_plan_validator_accepts_complete_visual_note() {
        let mut plan = seed_plan_with_frontend();
        plan.frontend.design.visual_notes.push(RegionDesignNote {
            region: "R1".into(),
            interpretation: "red box around delete = destructive emphasis".into(),
        });
        let report = validate_frontend_plan(&plan);
        assert!(report.passed, "{:?}", report.checks);
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
                    body_sketch:
                        "Generate id, build Expense, persist via store.save_expense, return rec."
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
        assert!(
            !wrapper_only_hit,
            "regression: wrapper alone now contains an anchor — test premise broken"
        );
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
        assert_eq!(f5.min_entities, 2); // 5 / 2
        assert_eq!(f5.min_tests, 5);
    }

    #[test]
    fn validate_backend_plan_enforces_region_floor_even_when_budget_low() {
        use crate::plan::{
            AppIdentity, BackendBudget, BackendSection, CanvasReference, CanvasRegion, CommandPlan,
            EntityPlan, FieldPlan, IterationPlan, PlanStage, TestPlan, PLAN_SCHEMA_VERSION,
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
                    motivated_by_regions: vec![
                        "R1".into(),
                        "R2".into(),
                        "R3".into(),
                        "R4".into(),
                        "R5".into(),
                    ],
                }],
                commands: vec![CommandPlan {
                    name: "list_tasks".into(),
                    input: "()".into(),
                    output: "Vec<Task>".into(),
                    motivated_by_regions: vec![
                        "R1".into(),
                        "R2".into(),
                        "R3".into(),
                        "R4".into(),
                        "R5".into(),
                    ],
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
            AppIdentity, BackendSection, CommandPlan, IterationPlan, PlanStage, PLAN_SCHEMA_VERSION,
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

    /// Minimal tokens.css body containing every REQUIRED_TOKEN_NAMES entry
    /// once, so lint tests that only care about app.css don't also trip
    /// the token-integrity gate.
    fn full_tokens_css() -> String {
        REQUIRED_TOKEN_NAMES
            .iter()
            .map(|name| format!("{name}: red;\n"))
            .collect::<String>()
    }

    /// Sets up `app/ui/styles/tokens.css` (complete) and
    /// `app/ui/styles/app.css` (given body) under a fresh worktree, plus
    /// the minimal interop/app.rs scaffolding `validate_frontend_impl`
    /// expects. Returns (worktree, job_dir) — job_dir has no baseline
    /// snapshot, so the growth gate reports "skipped".
    fn layered_worktree(app_css_body: &str) -> (tempfile::TempDir, IterationPlan) {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path().join("wt");
        let ui_src = worktree.join("app/ui/src");
        std::fs::create_dir_all(&ui_src).unwrap();
        std::fs::write(ui_src.join("interop.rs"), "").unwrap();
        std::fs::write(ui_src.join("app.rs"), "fn Home() {}\n").unwrap();

        let styles_dir = worktree.join("app/ui/styles");
        std::fs::create_dir_all(&styles_dir).unwrap();
        std::fs::write(styles_dir.join("tokens.css"), full_tokens_css()).unwrap();
        std::fs::write(styles_dir.join("app.css"), app_css_body).unwrap();

        let plan = seed_plan();
        (tmp, plan)
    }

    #[test]
    fn app_css_lint_rejects_hex_color_with_line_number() {
        let (tmp, plan) =
            layered_worktree(".widget {\n  color: #ff00aa;\n  font-size: var(--text-sm);\n}\n");
        let worktree = tmp.path().join("wt");
        let job_dir = tmp.path().join("job");
        let report = validate_frontend_impl(&plan, &worktree, &job_dir);
        let check = report
            .checks
            .iter()
            .find(|c| c.name == "styles/app.css: no hex color literals")
            .expect("hex lint check present");
        assert!(!check.passed);
        assert!(
            check.detail.contains("line 2"),
            "detail should cite the line number: {}",
            check.detail
        );
        assert!(check.detail.contains("#ff00aa"), "{}", check.detail);
    }

    #[test]
    fn app_css_lint_rejects_rgb_literal() {
        let (tmp, plan) = layered_worktree(
            ".widget {\n  background: rgba(255, 0, 0, 0.5);\n  font-size: var(--text-sm);\n}\n",
        );
        let worktree = tmp.path().join("wt");
        let job_dir = tmp.path().join("job");
        let report = validate_frontend_impl(&plan, &worktree, &job_dir);
        let check = report
            .checks
            .iter()
            .find(|c| c.name == "styles/app.css: no rgb()/rgba()/hsl() literals")
            .expect("color-function lint check present");
        assert!(!check.passed);
        assert!(check.detail.contains("line 2"), "{}", check.detail);
    }

    #[test]
    fn app_css_lint_rejects_untokened_font_size() {
        let (tmp, plan) = layered_worktree(".widget {\n  font-size: 14px;\n}\n");
        let worktree = tmp.path().join("wt");
        let job_dir = tmp.path().join("job");
        let report = validate_frontend_impl(&plan, &worktree, &job_dir);
        let check = report
            .checks
            .iter()
            .find(|c| c.name == "styles/app.css: font-size uses var(--text-*)")
            .expect("font-size lint check present");
        assert!(!check.passed);
        assert!(check.detail.contains("line 2"), "{}", check.detail);

        // A tokened font-size passes.
        let (tmp2, plan2) = layered_worktree(".widget {\n  font-size: var(--text-sm);\n}\n");
        let worktree2 = tmp2.path().join("wt");
        let job_dir2 = tmp2.path().join("job");
        let report2 = validate_frontend_impl(&plan2, &worktree2, &job_dir2);
        let check2 = report2
            .checks
            .iter()
            .find(|c| c.name == "styles/app.css: font-size uses var(--text-*)")
            .expect("font-size lint check present");
        assert!(check2.passed, "{:?}", check2);
    }

    #[test]
    fn app_css_lint_requires_focus_visible_rule() {
        let (tmp, plan) = layered_worktree(
            ".widget {\n  font-size: var(--text-sm);\n}\n.widget:hover {\n  color: var(--accent);\n}\n",
        );
        let worktree = tmp.path().join("wt");
        let job_dir = tmp.path().join("job");
        let report = validate_frontend_impl(&plan, &worktree, &job_dir);
        let hover_check = report
            .checks
            .iter()
            .find(|c| c.name == "styles/app.css: has a :hover rule")
            .expect(":hover check present");
        assert!(hover_check.passed);
        let focus_check = report
            .checks
            .iter()
            .find(|c| c.name == "styles/app.css: has a :focus-visible rule")
            .expect(":focus-visible check present");
        assert!(
            !focus_check.passed,
            "no :focus-visible rule should fail the floor"
        );
    }

    #[test]
    fn app_css_lint_ignores_hex_colors_inside_comments() {
        let (tmp, plan) = layered_worktree(
            "/* old color was #ff00aa, now tokenised */\n.widget {\n  color: var(--fg);\n  font-size: var(--text-sm);\n}\n.widget:hover { color: var(--accent); }\n.widget:focus-visible { outline: 2px solid var(--accent); }\n",
        );
        let worktree = tmp.path().join("wt");
        let job_dir = tmp.path().join("job");
        let report = validate_frontend_impl(&plan, &worktree, &job_dir);
        let check = report
            .checks
            .iter()
            .find(|c| c.name == "styles/app.css: no hex color literals")
            .expect("hex lint check present");
        assert!(
            check.passed,
            "hex literal inside a comment must not trip the lint: {:?}",
            check
        );
    }

    #[test]
    fn app_css_lint_exempts_custom_property_definitions() {
        // A `:root { --accent: #7c3aed; }` block at the top of app.css is
        // exactly how `theme_overrides` get realised — defining a token's
        // *value* with a raw color is the point, not a lint violation.
        let (tmp, plan) = layered_worktree(
            ":root {\n  --accent: #7c3aed;\n  --shadow-1: rgba(0, 0, 0, 0.2);\n}\n.widget {\n  color: var(--accent);\n  font-size: var(--text-sm);\n}\n.widget:hover { color: var(--accent); }\n.widget:focus-visible { outline: 2px solid var(--accent); }\n",
        );
        let worktree = tmp.path().join("wt");
        let job_dir = tmp.path().join("job");
        let report = validate_frontend_impl(&plan, &worktree, &job_dir);
        let hex_check = report
            .checks
            .iter()
            .find(|c| c.name == "styles/app.css: no hex color literals")
            .expect("hex lint check present");
        assert!(
            hex_check.passed,
            "token definition line must be exempt from hex lint: {:?}",
            hex_check
        );
        let rgb_check = report
            .checks
            .iter()
            .find(|c| c.name == "styles/app.css: no rgb()/rgba()/hsl() literals")
            .expect("rgb lint check present");
        assert!(
            rgb_check.passed,
            "token definition line must be exempt from rgb lint: {:?}",
            rgb_check
        );
    }

    #[test]
    fn app_css_lint_still_rejects_raw_color_in_non_property_declaration() {
        // The exemption is narrow: a normal `color: #7c3aed;` declaration
        // (not a `--name: value;` custom property) still fails.
        let (tmp, plan) = layered_worktree(
            ".widget {\n  color: #7c3aed;\n  font-size: var(--text-sm);\n}\n.widget:hover { color: var(--accent); }\n.widget:focus-visible { outline: 2px solid var(--accent); }\n",
        );
        let worktree = tmp.path().join("wt");
        let job_dir = tmp.path().join("job");
        let report = validate_frontend_impl(&plan, &worktree, &job_dir);
        let hex_check = report
            .checks
            .iter()
            .find(|c| c.name == "styles/app.css: no hex color literals")
            .expect("hex lint check present");
        assert!(
            !hex_check.passed,
            "a raw hex value on a normal property must still fail"
        );
    }

    #[test]
    fn theme_overrides_coherence_requires_reapplication_in_app_css() {
        let (tmp, mut plan) = layered_worktree(
            ".widget {\n  color: var(--fg);\n  font-size: var(--text-sm);\n}\n.widget:hover { color: var(--accent); }\n.widget:focus-visible { outline: 2px solid var(--accent); }\n",
        );
        plan.frontend.design.theme_overrides.push(TokenOverride {
            token: "--accent".into(),
            value: "#7c3aed".into(),
            reason: "user drew the header purple (R2)".into(),
        });
        let worktree = tmp.path().join("wt");
        let job_dir = tmp.path().join("job");
        let report = validate_frontend_impl(&plan, &worktree, &job_dir);
        let check = report
            .checks
            .iter()
            .find(|c| c.name == "design.themeOverrides applied in styles/app.css")
            .expect("coherence check present");
        assert!(!check.passed);
        assert!(check.detail.contains("--accent"), "{}", check.detail);
    }

    #[test]
    fn theme_overrides_coherence_passes_when_reapplied() {
        let (tmp, mut plan) = layered_worktree(
            ":root {\n  --accent: #7c3aed;\n}\n.widget {\n  color: var(--accent);\n  font-size: var(--text-sm);\n}\n.widget:hover { color: var(--accent); }\n.widget:focus-visible { outline: 2px solid var(--accent); }\n",
        );
        plan.frontend.design.theme_overrides.push(TokenOverride {
            token: "--accent".into(),
            value: "#7c3aed".into(),
            reason: "user drew the header purple (R2)".into(),
        });
        let worktree = tmp.path().join("wt");
        let job_dir = tmp.path().join("job");
        let report = validate_frontend_impl(&plan, &worktree, &job_dir);
        let check = report
            .checks
            .iter()
            .find(|c| c.name == "design.themeOverrides applied in styles/app.css")
            .expect("coherence check present");
        assert!(check.passed, "{:?}", check);
    }

    #[test]
    fn app_css_lint_full_pass_on_clean_stylesheet() {
        let (tmp, plan) = layered_worktree(
            ".widget {\n  color: var(--fg);\n  font-size: var(--text-sm);\n  box-shadow: var(--shadow-1);\n}\n.widget:hover { color: var(--accent); }\n.widget:focus-visible { outline: 2px solid var(--accent); }\n",
        );
        let worktree = tmp.path().join("wt");
        let job_dir = tmp.path().join("job");
        let report = validate_frontend_impl(&plan, &worktree, &job_dir);
        for name in [
            "styles/app.css: no hex color literals",
            "styles/app.css: no rgb()/rgba()/hsl() literals",
            "styles/app.css: font-size uses var(--text-*)",
            "styles/app.css: box-shadow uses var(--shadow-*)",
            "styles/app.css: has a :hover rule",
            "styles/app.css: has a :focus-visible rule",
            "styles/tokens.css exists",
            "styles/tokens.css defines every required token",
        ] {
            let check = report
                .checks
                .iter()
                .find(|c| c.name == name)
                .unwrap_or_else(|| panic!("missing check {name}"));
            assert!(check.passed, "{name} should pass: {:?}", check);
        }
    }

    #[test]
    fn tokens_css_missing_required_name_fails_integrity_gate() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path().join("wt");
        let ui_src = worktree.join("app/ui/src");
        std::fs::create_dir_all(&ui_src).unwrap();
        std::fs::write(ui_src.join("interop.rs"), "").unwrap();
        std::fs::write(ui_src.join("app.rs"), "fn Home() {}\n").unwrap();

        let styles_dir = worktree.join("app/ui/styles");
        std::fs::create_dir_all(&styles_dir).unwrap();
        // Drop --shadow-2 from an otherwise-complete tokens file.
        let incomplete: String = REQUIRED_TOKEN_NAMES
            .iter()
            .filter(|n| **n != "--shadow-2")
            .map(|name| format!("{name}: red;\n"))
            .collect();
        std::fs::write(styles_dir.join("tokens.css"), incomplete).unwrap();
        std::fs::write(
            styles_dir.join("app.css"),
            ".widget:hover{} .widget:focus-visible{}",
        )
        .unwrap();

        let plan = seed_plan();
        let job_dir = tmp.path().join("job");
        let report = validate_frontend_impl(&plan, &worktree, &job_dir);
        let check = report
            .checks
            .iter()
            .find(|c| c.name == "styles/tokens.css defines every required token")
            .expect("token integrity check present");
        assert!(!check.passed);
        assert!(check.detail.contains("--shadow-2"), "{}", check.detail);
    }

    #[test]
    fn frontend_impl_legacy_worktree_without_app_css_skips_token_lint() {
        // No app/ui/styles/app.css and no app/ui/styles/tokens.css — the
        // pre-split legacy layout. The token lint block must not fire at
        // all (no checks with the "styles/app.css:" or "styles/tokens.css"
        // prefixes), and the growth gate must fall back to the monolithic
        // styles.css, which `frontend_impl_accepts_grown_styles` already
        // covers for the passing case.
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path().join("wt");
        let ui_src = worktree.join("app/ui/src");
        std::fs::create_dir_all(&ui_src).unwrap();
        std::fs::write(ui_src.join("interop.rs"), "").unwrap();
        std::fs::write(ui_src.join("app.rs"), "fn Home() {}\n").unwrap();
        std::fs::write(
            worktree.join("app/ui/styles.css"),
            "body { color: black; }\n",
        )
        .unwrap();

        let plan = seed_plan();
        let job_dir = tmp.path().join("job");
        let report = validate_frontend_impl(&plan, &worktree, &job_dir);
        assert!(
            !report
                .checks
                .iter()
                .any(|c| c.name.starts_with("styles/app.css")
                    || c.name.starts_with("styles/tokens.css")),
            "legacy worktree must not run the layered token lint: {:?}",
            report.checks
        );
        let exists_check = report
            .checks
            .iter()
            .find(|c| c.name == "app/ui/styles.css exists")
            .expect("legacy exists check present");
        assert!(exists_check.passed);
    }

    #[test]
    fn final_review_requires_tokens_css_and_required_names() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path();
        let ui_dir = worktree.join("app/ui/src");
        std::fs::create_dir_all(&ui_dir).unwrap();
        std::fs::write(
            ui_dir.join("shell.rs"),
            "pub fn Shell() { let panel_open = RwSignal::new(false); FeedbackFab(); }",
        )
        .unwrap();
        std::fs::write(ui_dir.join("app.rs"), "view! { <Shell>..</Shell> }").unwrap();
        std::fs::write(
            worktree.join("app/ui/styles.css"),
            ".canvas-overlay { background: transparent; }",
        )
        .unwrap();
        let styles_dir = worktree.join("app/ui/styles");
        std::fs::create_dir_all(&styles_dir).unwrap();
        // Incomplete tokens.css — missing --radius-full.
        let incomplete: String = REQUIRED_TOKEN_NAMES
            .iter()
            .filter(|n| **n != "--radius-full")
            .map(|name| format!("{name}: 1px;\n"))
            .collect();
        std::fs::write(styles_dir.join("tokens.css"), incomplete).unwrap();

        let mut plan = crate::plan::IterationPlan::default();
        plan.app.iteration = 2;
        plan.stage = crate::plan::PlanStage::E2EImplemented;
        let report = validate_final(&plan, worktree);
        let check = report
            .checks
            .iter()
            .find(|c| c.name == "styles/tokens.css defines every required token")
            .expect("final review must check token integrity");
        assert!(!check.passed);
        assert!(check.detail.contains("--radius-full"), "{}", check.detail);

        // Complete tokens.css → green.
        std::fs::write(styles_dir.join("tokens.css"), full_tokens_css()).unwrap();
        let report = validate_final(&plan, worktree);
        let check = report
            .checks
            .iter()
            .find(|c| c.name == "styles/tokens.css defines every required token")
            .expect("final review must check token integrity");
        assert!(check.passed, "{:?}", check);
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

    #[test]
    fn final_review_skips_trunk_build_when_stack_rewritten() {
        // No app/ui/Trunk.toml in the worktree → the trunk gate records a
        // pass with an explicit "skipped" detail instead of failing a
        // stack it can't recognise.
        let tmp = tempfile::tempdir().unwrap();
        let mut plan = crate::plan::IterationPlan::default();
        plan.app.iteration = 2;
        plan.stage = crate::plan::PlanStage::E2EImplemented;
        let report = validate_final(&plan, tmp.path());
        let trunk = report
            .checks
            .iter()
            .find(|c| c.name.contains("trunk build"))
            .expect("trunk gate must always report");
        assert!(trunk.passed);
        assert!(trunk.detail.contains("skipped"), "{}", trunk.detail);
    }

    #[test]
    fn stub_smell_catches_fill_markers() {
        assert!(has_stub_smell("fn x() { // FILL: real query here\n }"));
        assert!(has_stub_smell("let q = \"FILL: where clause\";"));
        assert!(!has_stub_smell("fn x() { fulfill_order(); }"));
    }

    #[test]
    fn read_tail_returns_last_chars_only() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("out.log");
        std::fs::write(&p, format!("{}END", "x".repeat(5000))).unwrap();
        let tail = read_tail(&p, 100);
        assert_eq!(tail.chars().count(), 100);
        assert!(tail.ends_with("END"));
        assert!(read_tail(&tmp.path().join("missing"), 100).is_empty());
    }

    #[test]
    fn final_review_invariant_gates_catch_missing_fab_and_overlay() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path();
        let ui_dir = worktree.join("app/ui/src");
        std::fs::create_dir_all(&ui_dir).unwrap();
        // shell.rs lost the FAB; app.rs forgot to mount Shell; styles
        // dropped the overlay. All three gates must go red.
        std::fs::write(ui_dir.join("shell.rs"), "pub fn Shell() {}").unwrap();
        std::fs::write(ui_dir.join("app.rs"), "fn HomePage() {}").unwrap();
        std::fs::write(worktree.join("app/ui/styles.css"), "body {}").unwrap();

        let mut plan = crate::plan::IterationPlan::default();
        plan.app.iteration = 2;
        plan.stage = crate::plan::PlanStage::E2EImplemented;
        let report = validate_final(&plan, worktree);
        for name in [
            "shell.rs keeps FeedbackFab",
            "app.rs still mounts",
            ".canvas-overlay styles present",
        ] {
            let check = report
                .checks
                .iter()
                .find(|c| c.name.contains(name))
                .unwrap_or_else(|| panic!("missing check {name}"));
            assert!(!check.passed, "{name} should be red: {}", check.detail);
        }

        // Restore the invariants — all three gates go green.
        std::fs::write(
            ui_dir.join("shell.rs"),
            "pub fn Shell() { let panel_open = RwSignal::new(false); FeedbackFab(); }",
        )
        .unwrap();
        std::fs::write(ui_dir.join("app.rs"), "view! { <Shell>..</Shell> }").unwrap();
        std::fs::write(
            worktree.join("app/ui/styles.css"),
            ".canvas-overlay { background: transparent; }",
        )
        .unwrap();
        let report = validate_final(&plan, worktree);
        for name in [
            "shell.rs keeps FeedbackFab",
            "app.rs still mounts",
            ".canvas-overlay styles present",
        ] {
            let check = report
                .checks
                .iter()
                .find(|c| c.name.contains(name))
                .unwrap();
            assert!(check.passed, "{name} should be green: {}", check.detail);
        }
    }

    #[test]
    fn final_review_rejects_missing_design_md() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path();
        // No shell.rs at all — the shell-invariant block short-circuits to
        // "reviewed by hand", isolating this test to the DESIGN.md gate.
        let plan = crate::plan::IterationPlan::default();
        let report = validate_final(&plan, worktree);
        let check = report
            .checks
            .iter()
            .find(|c| c.name.contains("DESIGN.md"))
            .expect("DESIGN.md check present");
        assert!(!check.passed);
        assert!(check.detail.contains("missing"), "{}", check.detail);
    }

    #[test]
    fn final_review_rejects_stub_design_md() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path();
        std::fs::write(
            worktree.join("DESIGN.md"),
            "# Design\n\n## Design decisions\n\n- todo\n",
        )
        .unwrap();
        let plan = crate::plan::IterationPlan::default();
        let report = validate_final(&plan, worktree);
        let check = report
            .checks
            .iter()
            .find(|c| c.name.contains("DESIGN.md"))
            .expect("DESIGN.md check present");
        assert!(!check.passed, "a short stub must fail the size floor");
        assert!(check.detail.contains("stub"), "{}", check.detail);
    }

    #[test]
    fn final_review_rejects_design_md_missing_decision_log() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path();
        // Long enough to clear the size floor, but no `## Design decisions`
        // header — padding the file doesn't satisfy the gate.
        let body = format!(
            "# Design\n\n{}\n",
            "Some prose about the design. ".repeat(30)
        );
        assert!(body.len() as u64 >= DESIGN_MD_MIN_BYTES);
        std::fs::write(worktree.join("DESIGN.md"), body).unwrap();
        let plan = crate::plan::IterationPlan::default();
        let report = validate_final(&plan, worktree);
        let check = report
            .checks
            .iter()
            .find(|c| c.name.contains("DESIGN.md"))
            .expect("DESIGN.md check present");
        assert!(!check.passed);
        assert!(
            check.detail.contains("Design decisions"),
            "{}",
            check.detail
        );
    }

    #[test]
    fn final_review_accepts_real_design_md() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path();
        let body = format!(
            "# Design\n\n{}\n\n## Design decisions\n\n- 2026-07-17 — initial baseline.\n",
            "Token vocabulary and layout pattern prose. ".repeat(20)
        );
        assert!(body.len() as u64 >= DESIGN_MD_MIN_BYTES);
        std::fs::write(worktree.join("DESIGN.md"), body).unwrap();
        let plan = crate::plan::IterationPlan::default();
        let report = validate_final(&plan, worktree);
        let check = report
            .checks
            .iter()
            .find(|c| c.name.contains("DESIGN.md"))
            .expect("DESIGN.md check present");
        assert!(check.passed, "{:?}", check);
    }

    #[test]
    fn build_gates_skip_cleanly_on_rewritten_stack() {
        // A worktree with no root Cargo.toml (fully rewritten stack) must
        // not be failed by gates that assume the stock layout.
        let tmp = tempfile::tempdir().unwrap();
        let report = validate_build_gates(tmp.path());
        assert!(report.passed);
        assert_eq!(report.checks.len(), 1);
        assert!(report.checks[0].detail.contains("skipped"));
    }
}
