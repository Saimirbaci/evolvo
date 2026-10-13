//! Launching the new version of an external app — the last step of
//! "evolve any app": capture → find/clone source → agent implements →
//! build → **run it**.
//!
//! How to build and run a foreign project is project-specific, so the agent
//! (which has just worked in the repo) declares it in a run spec
//! (`<job dir>/run_spec.json`, see `prompts/external_app.md`). When it
//! doesn't, [`detect_run_spec`] guesses from the manifests. The spec's
//! build steps double as the job's build gate (`external::validate_external_gates`),
//! so by the time a job is Ready for review the run step starts instantly.
//!
//! Launched runs are tracked per job (process group id) so they can be
//! stopped, relaunched without duplicates, and — via [`job_for_pid`] —
//! recognised when the user presses the desktop shortcut on the new build,
//! making follow-up feedback continue from that job's worktree.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::lineage::LineageEngine;
use crate::store::Store;

pub const RUN_SPEC_FILE: &str = "run_spec.json";

/// How to build and start the app from its worktree.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct RunSpec {
    /// Commands run in order before launching (and used as the build gate),
    /// e.g. `[["cargo","build"]]` or `[["pnpm","install"],["pnpm","run","build"]]`.
    #[serde(default)]
    pub build: Vec<Vec<String>>,
    /// The long-running command that starts the app, e.g. `["cargo","run"]`.
    pub run: Vec<String>,
    /// Working directory relative to the worktree root (default: root).
    #[serde(default)]
    pub cwd: Option<String>,
    /// Free text shown to the reviewer ("first build takes ~15 min").
    #[serde(default)]
    pub notes: String,
    /// `"agent"` (declared in run_spec.json) or `"detected"`.
    #[serde(default)]
    pub source: String,
}

impl RunSpec {
    /// Structural validation of an agent-written spec: non-empty argv
    /// entries without NUL bytes, at most 6 build steps, and a `cwd` that
    /// stays inside the worktree.
    pub fn validate(&self) -> Result<(), String> {
        let argv_ok = |argv: &[String]| {
            !argv.is_empty()
                && argv.len() <= 64
                && argv.iter().all(|a| !a.contains('\0'))
                && !argv[0].trim().is_empty()
        };
        if !argv_ok(&self.run) {
            return Err("run command is empty or malformed".into());
        }
        if self.build.len() > 6 || !self.build.iter().all(|b| argv_ok(b)) {
            return Err("build steps are malformed".into());
        }
        if let Some(cwd) = self.cwd.as_deref() {
            let p = Path::new(cwd);
            if p.is_absolute()
                || p.components()
                    .any(|c| matches!(c, Component::ParentDir | Component::Prefix(_)))
            {
                return Err("cwd must be a path inside the worktree".into());
            }
        }
        Ok(())
    }

    pub fn working_dir(&self, worktree: &Path) -> PathBuf {
        match self.cwd.as_deref().filter(|c| !c.trim().is_empty()) {
            Some(c) => worktree.join(c),
            None => worktree.to_path_buf(),
        }
    }

    pub fn describe(&self) -> String {
        self.run.join(" ")
    }
}

/// The agent-declared spec for a job, if present and valid.
pub fn load_run_spec(job_dir: &Path) -> Option<RunSpec> {
    let bytes = std::fs::read(job_dir.join(RUN_SPEC_FILE)).ok()?;
    let mut spec: RunSpec = serde_json::from_slice(&bytes).ok()?;
    spec.validate().ok()?;
    spec.source = "agent".into();
    Some(spec)
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

/// Guess build/run commands from the repo's manifests.
pub fn detect_run_spec(worktree: &Path) -> Option<RunSpec> {
    let spec = |build: Vec<Vec<String>>, run: Vec<String>| RunSpec {
        build,
        run,
        cwd: None,
        notes: String::new(),
        source: "detected".into(),
    };
    if worktree.join("Cargo.toml").is_file() {
        return Some(spec(
            vec![argv(&["cargo", "build"])],
            argv(&["cargo", "run"]),
        ));
    }
    if worktree.join("package.json").is_file() {
        let pkg: Value = std::fs::read_to_string(worktree.join("package.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(Value::Null);
        let scripts = pkg.get("scripts");
        let has = |name: &str| scripts.and_then(|s| s.get(name)).is_some();
        let pm = if worktree.join("pnpm-lock.yaml").is_file() {
            "pnpm"
        } else if worktree.join("yarn.lock").is_file() {
            "yarn"
        } else if worktree.join("bun.lockb").is_file() || worktree.join("bun.lock").is_file() {
            "bun"
        } else {
            "npm"
        };
        let script = ["dev", "start"].into_iter().find(|s| has(s))?;
        let mut build = vec![argv(&[pm, "install"])];
        if has("build") {
            build.push(argv(&[pm, "run", "build"]));
        }
        return Some(spec(build, argv(&[pm, "run", script])));
    }
    if worktree.join("go.mod").is_file() {
        return Some(spec(
            vec![argv(&["go", "build", "./..."])],
            argv(&["go", "run", "."]),
        ));
    }
    None
}

/// Agent-declared spec, else detected.
pub fn resolve_run_spec(job_dir: &Path, worktree: &Path) -> Option<RunSpec> {
    load_run_spec(job_dir).or_else(|| detect_run_spec(worktree))
}

// ---------------------------------------------------------------------
// Running instances
// ---------------------------------------------------------------------

/// job id → process-group id of its launched app.
fn running() -> &'static Mutex<HashMap<String, u32>> {
    static MAP: OnceLock<Mutex<HashMap<String, u32>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

fn pid_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Whether this job's launched app is still running.
pub fn is_running(job_id: &str) -> bool {
    let pid = running().lock().ok().and_then(|m| m.get(job_id).copied());
    pid.is_some_and(pid_alive)
}

/// Stop a job's launched app (whole process group). Returns whether one
/// was running.
pub fn stop(job_id: &str) -> bool {
    let pid = running().lock().ok().and_then(|mut m| m.remove(job_id));
    match pid {
        Some(pgid) if pid_alive(pgid) => {
            let _ = Command::new("kill")
                .args(["-TERM", &format!("-{pgid}")])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            true
        }
        _ => false,
    }
}

fn parent_pid(pid: u32) -> Option<u32> {
    let out = Command::new("ps")
        .args(["-o", "ppid=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// The job whose launched run owns `pid` — the process itself or one of
/// its ancestors (`cargo run` → app) is a tracked process-group leader.
pub fn job_for_pid(pid: u32) -> Option<String> {
    let map = running().lock().ok()?.clone();
    if map.is_empty() {
        return None;
    }
    let mut current = Some(pid);
    for _ in 0..6 {
        let p = current?;
        if p <= 1 {
            return None;
        }
        if let Some((job, _)) = map.iter().find(|(_, pgid)| **pgid == p) {
            return Some(job.clone());
        }
        current = parent_pid(p);
    }
    None
}

/// If `target` is a version Evolvo launched for an earlier job, point the
/// new request at that job's worktree (its unmerged changes included) so
/// follow-up feedback continues from it instead of re-resolving the
/// upstream repo. Returns the job id when linked.
pub fn link_launched_build(store: &Store, target: &mut crate::types::TargetApp) -> Option<String> {
    let job_id = job_for_pid(target.pid?)?;
    let job = store.load_lineage_job(&job_id).ok().flatten()?;
    let worktree = job.worktree_path.filter(|w| Path::new(w).is_dir())?;
    // Keep the upstream identity of the original target for the prompt.
    if let Some(original) = store
        .load_feedback(&job.feedback_id)
        .ok()
        .flatten()
        .and_then(|f| f.target_app)
    {
        if target.repo_url.is_none() {
            target.repo_url = original.repo_url;
        }
        if target.bundle_id.is_none() {
            target.bundle_id = original.bundle_id;
        }
    }
    target.repo_path = Some(worktree);
    target.repo_source = "evolvo-build".into();
    target.repo_status = "resolved".into();
    target.repo_error = None;
    target.based_on_job = Some(job_id.clone());
    Some(job_id)
}

/// Start the app for an external-app job from its worktree: run the build
/// steps (skipped when `skip_build`, i.e. the gate just ran them), then the
/// run command in its own process group, logging to `iteration-run.log`.
/// Replaces any instance this job already has running.
pub fn launch(store: Store, job_id: String, skip_build: bool) {
    std::thread::spawn(move || {
        let engine = LineageEngine::new(&store);
        let note = |m: &str| {
            let _ = engine.append_note(&job_id, m);
        };
        let Some(job) = store.load_lineage_job(&job_id).ok().flatten() else {
            return;
        };
        let Some(worktree) = job
            .worktree_path
            .as_deref()
            .map(PathBuf::from)
            .filter(|w| w.is_dir())
        else {
            note("launch skipped — the job's worktree is missing");
            return;
        };
        let root = store.layout().root().to_path_buf();
        let job_dir = crate::runner::job_workspace_dir(&root, &job_id);
        let app_name = store
            .load_feedback(&job.feedback_id)
            .ok()
            .flatten()
            .and_then(|f| f.target_app.map(|t| t.name))
            .unwrap_or_else(|| "the app".into());
        let Some(spec) = resolve_run_spec(&job_dir, &worktree) else {
            note("launch skipped — couldn't tell how to run this project (no run_spec.json from the agent and no Cargo.toml / package.json dev script / go.mod)");
            crate::notify::desktop_notify(
                &format!("Evolvo — {app_name} is ready"),
                "Built, but Evolvo doesn't know how to start it — see the job notes",
            );
            return;
        };
        let cwd = spec.working_dir(&worktree);
        let log_path = crate::runner::iteration_run_log_path(&root, &job_id);
        let Ok(log) = std::fs::File::create(&log_path) else {
            note(&format!(
                "launch failed — cannot open {}",
                log_path.display()
            ));
            return;
        };
        let _ = writeln!(
            &log,
            "== new version of {app_name} ==\njob: {job_id}\ncwd: {}\nrun: {}\nsource: {}\n--",
            cwd.display(),
            spec.describe(),
            spec.source
        );

        if stop(&job_id) {
            note("stopped the previously launched version");
        }

        if !skip_build {
            for step in &spec.build {
                note(&format!("building: {}", step.join(" ")));
                let mut cmd = Command::new(&step[0]);
                cmd.args(&step[1..])
                    .current_dir(&cwd)
                    .env("PATH", crate::runner::enriched_path())
                    .stdin(Stdio::null())
                    .stdout(
                        log.try_clone()
                            .map(Stdio::from)
                            .unwrap_or_else(|_| Stdio::null()),
                    )
                    .stderr(
                        log.try_clone()
                            .map(Stdio::from)
                            .unwrap_or_else(|_| Stdio::null()),
                    );
                match crate::proc::run_with_timeout(&mut cmd, crate::external::gate_timeout()) {
                    Ok(crate::proc::WaitOutcome::Exited(s)) if s.success() => {}
                    other => {
                        let why = match other {
                            Ok(crate::proc::WaitOutcome::TimedOut) => "timed out".to_string(),
                            Ok(crate::proc::WaitOutcome::Exited(s)) => format!("exited {s}"),
                            Err(e) => e.to_string(),
                        };
                        note(&format!(
                            "launch failed — build step `{}` {why}; see {}",
                            step.join(" "),
                            log_path.display()
                        ));
                        crate::notify::desktop_notify(
                            &format!("Evolvo — couldn't start {app_name}"),
                            &format!("Build step `{}` {why}", step.join(" ")),
                        );
                        return;
                    }
                }
            }
        }

        let mut cmd = Command::new(&spec.run[0]);
        cmd.args(&spec.run[1..])
            .current_dir(&cwd)
            .env("PATH", crate::runner::enriched_path())
            .stdin(Stdio::null())
            .stdout(
                log.try_clone()
                    .map(Stdio::from)
                    .unwrap_or_else(|_| Stdio::null()),
            )
            .stderr(
                log.try_clone()
                    .map(Stdio::from)
                    .unwrap_or_else(|_| Stdio::null()),
            );
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // Own process group: Stop reaches the app, not just `cargo run`.
            cmd.process_group(0);
        }
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                note(&format!(
                    "launch failed — could not start `{}`: {e}",
                    spec.describe()
                ));
                crate::notify::desktop_notify(
                    &format!("Evolvo — couldn't start {app_name}"),
                    &format!("`{}`: {e}", spec.describe()),
                );
                return;
            }
        };
        let pgid = child.id();
        if let Ok(mut m) = running().lock() {
            m.insert(job_id.clone(), pgid);
        }
        crate::tray::request_refresh();
        note(&format!(
            "launched the new version: `{}` in {} (pid {pgid}) — log {}",
            spec.describe(),
            cwd.display(),
            log_path.display()
        ));
        crate::notify::desktop_notify(
            &format!("Evolvo — new {app_name} is starting"),
            "Try your change. Press the feedback shortcut on it to keep iterating.",
        );

        let started = std::time::Instant::now();
        let status = child.wait();
        let still_ours = running()
            .lock()
            .map(|m| m.get(&job_id) == Some(&pgid))
            .unwrap_or(false);
        if still_ours {
            if let Ok(mut m) = running().lock() {
                m.remove(&job_id);
            }
            crate::tray::request_refresh();
            let quick_fail = started.elapsed() < std::time::Duration::from_secs(30)
                && !matches!(status, Ok(s) if s.success());
            let msg = match &status {
                Ok(s) => format!(
                    "the launched version exited ({s}) — log {}",
                    log_path.display()
                ),
                Err(e) => format!("the launched version ended: {e}"),
            };
            note(&msg);
            if quick_fail {
                crate::notify::desktop_notify(
                    &format!("Evolvo — {app_name} stopped right away"),
                    "See the job's run log on the Lineage page",
                );
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_rejects_escaping_cwd_and_empty_commands() {
        let ok = RunSpec {
            run: argv(&["cargo", "run"]),
            cwd: Some("apps/desktop".into()),
            ..Default::default()
        };
        assert!(ok.validate().is_ok());
        for cwd in ["../elsewhere", "/etc", "a/../../b"] {
            let bad = RunSpec {
                cwd: Some(cwd.into()),
                ..ok.clone()
            };
            assert!(bad.validate().is_err(), "{cwd}");
        }
        let empty = RunSpec::default();
        assert!(empty.validate().is_err());
        let bad_build = RunSpec {
            build: vec![vec![]],
            ..ok.clone()
        };
        assert!(bad_build.validate().is_err());
    }

    #[test]
    fn detect_by_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path();
        assert!(detect_run_spec(p).is_none());
        std::fs::write(
            p.join("package.json"),
            r#"{"scripts":{"build":"vite build"}}"#,
        )
        .unwrap();
        assert!(
            detect_run_spec(p).is_none(),
            "no dev/start script → can't run"
        );
        std::fs::write(
            p.join("package.json"),
            r#"{"scripts":{"dev":"vite","build":"vite build"}}"#,
        )
        .unwrap();
        std::fs::write(p.join("pnpm-lock.yaml"), "").unwrap();
        let s = detect_run_spec(p).unwrap();
        assert_eq!(s.run, argv(&["pnpm", "run", "dev"]));
        assert_eq!(
            s.build,
            vec![argv(&["pnpm", "install"]), argv(&["pnpm", "run", "build"])]
        );
        std::fs::write(p.join("Cargo.toml"), "[workspace]\n").unwrap();
        assert_eq!(detect_run_spec(p).unwrap().run, argv(&["cargo", "run"]));
    }

    #[test]
    fn agent_spec_wins_and_invalid_spec_is_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let (job_dir, wt) = (tmp.path().join("job"), tmp.path().join("wt"));
        std::fs::create_dir_all(&job_dir).unwrap();
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join("Cargo.toml"), "[package]\n").unwrap();
        std::fs::write(
            job_dir.join(RUN_SPEC_FILE),
            r#"{"build":[["cargo","build","-p","zed"]],"run":["cargo","run","-p","zed"],"notes":"slow first build"}"#,
        )
        .unwrap();
        let s = resolve_run_spec(&job_dir, &wt).unwrap();
        assert_eq!((s.source.as_str(), s.run[3].as_str()), ("agent", "zed"));
        std::fs::write(job_dir.join(RUN_SPEC_FILE), r#"{"run":[],"cwd":"../x"}"#).unwrap();
        assert_eq!(resolve_run_spec(&job_dir, &wt).unwrap().source, "detected");
    }

    #[test]
    fn launched_runs_are_found_by_pid_ancestry_and_stoppable() {
        // A real process group: `sh` sleeping, tracked as job-pid.
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "sleep 30"]);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let mut child = cmd.spawn().unwrap();
        let pgid = child.id();
        running().lock().unwrap().insert("job-pid".into(), pgid);
        assert!(is_running("job-pid"));
        assert_eq!(job_for_pid(pgid).as_deref(), Some("job-pid"));
        assert_eq!(job_for_pid(std::process::id()), None);
        assert!(stop("job-pid"));
        let _ = child.wait();
        assert!(!is_running("job-pid"));
    }

    #[test]
    fn job_for_pid_walks_up_from_a_child_process() {
        // `npm run dev`-style launch: the app is a child of the tracked
        // process-group leader, not the leader itself.
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "sleep 30 & wait"]);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let mut child = cmd.spawn().unwrap();
        let pgid = child.id();
        running().lock().unwrap().insert("job-tree".into(), pgid);
        let mut app_pid = None;
        for _ in 0..50 {
            let out = Command::new("pgrep")
                .args(["-P", &pgid.to_string()])
                .output()
                .unwrap();
            app_pid = String::from_utf8_lossy(&out.stdout)
                .lines()
                .next()
                .and_then(|l| l.trim().parse::<u32>().ok());
            if app_pid.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let app_pid = app_pid.expect("sleep child spawned");
        assert_ne!(app_pid, pgid);
        assert_eq!(job_for_pid(app_pid).as_deref(), Some("job-tree"));
        assert!(stop("job-tree"));
        let _ = child.wait();
    }
}
