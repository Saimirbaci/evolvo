//! `evolvo_desktop validate` — the stage validators exposed as a CLI.
//!
//! This is the "validator as a tool" half of the self-correction loop: the
//! same deterministic gate that `stages.rs` applies after a stage session
//! exits is callable *during* the session, so the agent can iterate until
//! green instead of discovering the rejection post-hoc. The runner passes
//! the host binary's path to every stage session as `$EVOLVO_VALIDATE_BIN`;
//! `prompts/stages/_self_validate.md` tells the agent how to invoke it.
//!
//! Usage:
//!
//! ```text
//! evolvo_desktop validate --stage backend_impl \
//!     --job-dir <job workspace dir containing plan.json> \
//!     --worktree <lineage worktree root> [--json]
//! ```
//!
//! Exit code 0 = the stage would be accepted; 1 = rejected (report on
//! stdout); 2 = usage / I/O error.

use std::path::PathBuf;

use crate::plan::load_plan;
use crate::stages::run_validator;
use crate::types::StageKind;
use crate::validators::StageReport;

pub const USAGE: &str = "usage: evolvo_desktop validate --stage <slug> --job-dir <dir> --worktree <dir> [--json]\n  slugs: backend_plan backend_impl frontend_plan frontend_impl e2e_plan e2e_impl final_review";

/// Parsed arguments for the `validate` subcommand.
#[derive(Debug, PartialEq, Eq)]
pub struct ValidateArgs {
    pub stage: StageKind,
    pub job_dir: PathBuf,
    pub worktree: PathBuf,
    pub json: bool,
}

/// Parse the argv slice *after* the `validate` token. Hand-rolled — three
/// flags don't justify a parser dependency.
pub fn parse_args(args: &[String]) -> Result<ValidateArgs, String> {
    let mut stage: Option<StageKind> = None;
    let mut job_dir: Option<PathBuf> = None;
    let mut worktree: Option<PathBuf> = None;
    let mut json = false;

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--stage" => {
                let v = it.next().ok_or("--stage requires a value")?;
                stage = Some(
                    StageKind::from_slug(v)
                        .ok_or_else(|| format!("unknown stage slug `{v}`\n{USAGE}"))?,
                );
            }
            "--job-dir" => {
                let v = it.next().ok_or("--job-dir requires a value")?;
                job_dir = Some(PathBuf::from(v));
            }
            "--worktree" => {
                let v = it.next().ok_or("--worktree requires a value")?;
                worktree = Some(PathBuf::from(v));
            }
            "--json" => json = true,
            other => return Err(format!("unknown argument `{other}`\n{USAGE}")),
        }
    }

    Ok(ValidateArgs {
        stage: stage.ok_or_else(|| format!("--stage is required\n{USAGE}"))?,
        job_dir: job_dir.ok_or_else(|| format!("--job-dir is required\n{USAGE}"))?,
        worktree: worktree.ok_or_else(|| format!("--worktree is required\n{USAGE}"))?,
        json,
    })
}

/// Load the plan and run the stage's validator. Errors are usage/I-O
/// problems (exit 2 territory), not validation rejections.
pub fn run(args: &ValidateArgs) -> Result<StageReport, String> {
    let plan = load_plan(&args.job_dir)
        .map_err(|e| {
            format!(
                "failed to read plan.json under {}: {e}",
                args.job_dir.display()
            )
        })?
        .ok_or_else(|| format!("no plan.json under {}", args.job_dir.display()))?;
    Ok(run_validator(
        args.stage,
        &plan,
        &args.worktree,
        &args.job_dir,
    ))
}

/// Full subcommand entry point: parse, run, print, and return the process
/// exit code. Kept free of `std::process::exit` so tests can call it.
pub fn main(args: &[String]) -> i32 {
    let parsed = match parse_args(args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    match run(&parsed) {
        Ok(report) => {
            if parsed.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report.to_json()).unwrap_or_default()
                );
            } else {
                print!("{}", report.render_text());
            }
            if report.passed {
                0
            } else {
                1
            }
        }
        Err(e) => {
            eprintln!("{e}");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parse_args_happy_path() {
        let args = parse_args(&s(&[
            "--stage",
            "backend_plan",
            "--job-dir",
            "/tmp/job",
            "--worktree",
            "/tmp/wt",
            "--json",
        ]))
        .unwrap();
        assert_eq!(args.stage, StageKind::BackendPlan);
        assert_eq!(args.job_dir, PathBuf::from("/tmp/job"));
        assert_eq!(args.worktree, PathBuf::from("/tmp/wt"));
        assert!(args.json);
    }

    #[test]
    fn parse_args_rejects_unknown_slug_and_missing_flags() {
        let err = parse_args(&s(&["--stage", "nonsense"])).unwrap_err();
        assert!(err.contains("unknown stage slug"));
        let err = parse_args(&s(&["--stage", "backend_plan"])).unwrap_err();
        assert!(err.contains("--job-dir is required"));
        let err = parse_args(&s(&["--bogus"])).unwrap_err();
        assert!(err.contains("unknown argument"));
    }

    #[test]
    fn run_reports_missing_plan_as_error_not_rejection() {
        let tmp = tempfile::tempdir().unwrap();
        let args = ValidateArgs {
            stage: StageKind::BackendPlan,
            job_dir: tmp.path().join("nope"),
            worktree: tmp.path().to_path_buf(),
            json: false,
        };
        let err = run(&args).unwrap_err();
        assert!(err.contains("no plan.json"));
    }

    #[test]
    fn main_exit_codes_distinguish_reject_from_usage_error() {
        // Usage error → 2.
        assert_eq!(main(&s(&["--stage", "bogus"])), 2);

        // A seeded (incomplete) plan → validator rejection → 1.
        let tmp = tempfile::tempdir().unwrap();
        let plan = crate::plan::IterationPlan::default();
        crate::plan::save_plan(tmp.path(), &plan).unwrap();
        let code = main(&s(&[
            "--stage",
            "backend_plan",
            "--job-dir",
            &tmp.path().display().to_string(),
            "--worktree",
            &tmp.path().display().to_string(),
        ]));
        assert_eq!(code, 1);
    }
}
