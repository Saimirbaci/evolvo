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

    let entity_count = plan.backend.entities.len() as u32;
    r.push(
        "entities meet budget",
        entity_count >= plan.backend.budget.min_entities.max(1),
        format!(
            "have {} entities (min {})",
            entity_count, plan.backend.budget.min_entities
        ),
    );
    let cmd_count = plan.backend.commands.len() as u32;
    r.push(
        "commands meet budget",
        cmd_count >= plan.backend.budget.min_commands.max(1),
        format!(
            "have {} commands (min {})",
            cmd_count, plan.backend.budget.min_commands
        ),
    );
    let test_count = plan.backend.tests.len() as u32;
    r.push(
        "tests meet budget",
        test_count >= plan.backend.budget.min_tests.max(1),
        format!(
            "have {} tests (min {})",
            test_count, plan.backend.budget.min_tests
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
            &format!("backend command `{}` registered in invoke_handler", cmd.name),
            has_registration,
            "search lib.rs + main.rs",
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
        Ok(false) => r.push("cargo test -p evolvo_desktop --lib", false, "exited non-zero"),
        Err(e) => r.push("cargo test -p evolvo_desktop --lib", false, e),
    }
    r.finalize()
}

pub fn validate_frontend_impl(plan: &IterationPlan, worktree: &Path) -> StageReport {
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

    match run_cargo(
        worktree,
        &["check", "-p", "evolvo_ui", "--target", "wasm32-unknown-unknown"],
    ) {
        Ok(true) => r.push("cargo check -p evolvo_ui (wasm)", true, "ok"),
        Ok(false) => r.push(
            "cargo check -p evolvo_ui (wasm)",
            false,
            "exited non-zero",
        ),
        Err(e) => r.push("cargo check -p evolvo_ui (wasm)", false, e),
    }
    r.finalize()
}

pub fn validate_e2e_impl(plan: &IterationPlan, worktree: &Path) -> StageReport {
    let mut r = StageReport::new("e2e_impl");
    let scripts = worktree.join("scripts");
    r.push(
        "scripts/run-iteration.sh exists OR default stack intact",
        scripts.join("run-iteration.sh").exists()
            || worktree.join("app/src-tauri").exists(),
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
pub fn validate_final(plan: &IterationPlan) -> StageReport {
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
    r.finalize()
}

// --- helpers ---

fn is_snake_case(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !s.starts_with('_')
        && !s.ends_with('_')
}

fn has_stub_smell(body: &str) -> bool {
    body.contains("TODO") || body.contains("unimplemented!") || body.contains("todo!()")
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
                    },
                    CommandPlan {
                        name: "list_projects".into(),
                        input: "()".into(),
                        output: "Vec<Project>".into(),
                        motivated_by_regions: vec!["R1".into()],
                        summary: "".into(),
                    },
                    CommandPlan {
                        name: "get_project".into(),
                        input: "EntityIdPayload".into(),
                        output: "Option<Project>".into(),
                        motivated_by_regions: vec!["R1".into()],
                        summary: "".into(),
                    },
                    CommandPlan {
                        name: "delete_project".into(),
                        input: "EntityIdPayload".into(),
                        output: "bool".into(),
                        motivated_by_regions: vec!["R1".into()],
                        summary: "".into(),
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
        assert!(report
            .checks
            .iter()
            .any(|c| c.name.contains("canvas region has at least one backend owner") && !c.passed));
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
    fn stage_report_finalize_sets_passed_when_all_green() {
        let mut r = StageReport::new("t");
        r.push("a", true, "ok");
        r.push("b", true, "ok");
        let r = r.finalize();
        assert!(r.passed);
        assert!(r.headline.contains("passed"));
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
}
