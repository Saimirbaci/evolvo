//! Runs Claude Code non-interactively inside an isolated git worktree
//! forked from the NoIDE source repo. This is the bridge between a reviewer
//! pressing "Advance" on a sandbox job and actual code being written.
//!
//! Safety posture:
//! - The worktree lives on its own branch (`sandbox/<job-id>`) so Claude
//!   can never touch the main branch or the primary checkout.
//! - Claude is launched with `--permission-mode acceptEdits`. It auto-
//!   approves file edits inside its sandbox but still pauses for genuinely
//!   risky operations. This is intentionally more conservative than
//!   `--dangerously-skip-permissions`.
//! - All stdout + stderr is streamed to `claude.log` under the job's sandbox
//!   workspace directory, and every state transition is appended to the job
//!   record's notes for observability.
//! - If `claude` or `git` is missing the job transitions to `Failed` with a
//!   note explaining how to fix the environment.
//!
//! Source repo resolution order:
//! 1. `NOIDE_SOURCE_REPO` env var.
//! 2. Walk up from `CARGO_MANIFEST_DIR` (compile-time) for a directory that
//!    has both `.git/` and `.claude/agents/`.
//! 3. Walk up from the process CWD using the same check.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::sandbox::SandboxEngine;
use crate::store::{Store, StoreError};
use crate::types::{FeedbackRecord, SandboxJobRecord, SandboxJobStatus};

const SANDBOX_WORKSPACES_DIR: &str = "sandbox_workspaces";
const WORKTREE_DIR: &str = "worktree";
const INPUTS_DIR: &str = "inputs";
const LOG_FILE: &str = "claude.log";
const PROMPT_FILE: &str = "prompt.md";
const METADATA_FILE: &str = "run.json";
const RUN_LOG_FILE: &str = "iteration-run.log";
const RUN_WORKSPACE_DIR: &str = "run_workspace";
const DEFAULT_RUN_SCRIPT: &str = "scripts/run-iteration.sh";

/// Locate the NoIDE source repo that should be forked into the sandbox.
pub fn resolve_source_repo() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("NOIDE_SOURCE_REPO") {
        let pb = PathBuf::from(p);
        if is_source_repo(&pb) {
            return Some(pb);
        }
    }

    // CARGO_MANIFEST_DIR points at app/src-tauri at compile time. The repo
    // root sits two levels up, which makes `cargo tauri dev` work with no
    // env setup.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Some(found) = walk_up_for_source(&manifest_dir) {
        return Some(found);
    }

    if let Ok(cwd) = std::env::current_dir() {
        if let Some(found) = walk_up_for_source(&cwd) {
            return Some(found);
        }
    }
    None
}

fn walk_up_for_source(start: &Path) -> Option<PathBuf> {
    let mut cur = start.to_path_buf();
    loop {
        if is_source_repo(&cur) {
            return Some(cur);
        }
        if !cur.pop() {
            return None;
        }
    }
}

fn is_source_repo(p: &Path) -> bool {
    p.join(".git").exists() && p.join(".claude").join("agents").exists()
}

pub fn sandbox_workspaces_root(workspace_root: &Path) -> PathBuf {
    workspace_root.join(SANDBOX_WORKSPACES_DIR)
}

pub fn job_workspace_dir(workspace_root: &Path, job_id: &str) -> PathBuf {
    sandbox_workspaces_root(workspace_root).join(job_id)
}

pub fn worktree_path(workspace_root: &Path, job_id: &str) -> PathBuf {
    job_workspace_dir(workspace_root, job_id).join(WORKTREE_DIR)
}

pub fn inputs_path(workspace_root: &Path, job_id: &str) -> PathBuf {
    job_workspace_dir(workspace_root, job_id).join(INPUTS_DIR)
}

pub fn log_path(workspace_root: &Path, job_id: &str) -> PathBuf {
    job_workspace_dir(workspace_root, job_id).join(LOG_FILE)
}

pub fn prompt_path(workspace_root: &Path, job_id: &str) -> PathBuf {
    job_workspace_dir(workspace_root, job_id).join(PROMPT_FILE)
}

pub fn metadata_path(workspace_root: &Path, job_id: &str) -> PathBuf {
    job_workspace_dir(workspace_root, job_id).join(METADATA_FILE)
}

pub fn branch_name(job_id: &str) -> String {
    format!("sandbox/{job_id}")
}

/// Tear down any previous worktree + branch + job workspace left over from an
/// earlier attempt, so `prepare_run` can start from a clean slate. All steps
/// are best-effort: missing worktrees/branches are not errors, but a failure
/// to remove an existing worktree (e.g. a lock held by another `git` process)
/// is surfaced so the caller can refuse to retry rather than silently drift.
pub fn cleanup_previous_run(
    source: &Path,
    workspace_root: &Path,
    job_id: &str,
) -> Result<(), StoreError> {
    let worktree = worktree_path(workspace_root, job_id);
    let branch = branch_name(job_id);

    if worktree.exists() {
        let output = Command::new("git")
            .arg("worktree")
            .arg("remove")
            .arg("--force")
            .arg(&worktree)
            .current_dir(source)
            .output()
            .map_err(|e| StoreError::Other(format!("failed to spawn git: {e}")))?;
        if !output.status.success() {
            // Fall back to a direct fs delete so a stale record in
            // `.git/worktrees/` can't permanently block retries. Git will
            // re-sync on the next `worktree add`.
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!(
                "git worktree remove exited {}: {} — falling back to fs delete",
                output.status,
                stderr.trim()
            );
            if worktree.exists() {
                fs::remove_dir_all(&worktree).ok();
            }
            let _ = Command::new("git")
                .arg("worktree")
                .arg("prune")
                .current_dir(source)
                .output();
        }
    }

    // `git branch -D` returns non-zero when the branch is missing; that's
    // fine — we're doing best-effort cleanup.
    let _ = Command::new("git")
        .arg("branch")
        .arg("-D")
        .arg(&branch)
        .current_dir(source)
        .output();

    let job_dir = job_workspace_dir(workspace_root, job_id);
    if job_dir.exists() {
        fs::remove_dir_all(&job_dir)?;
    }
    Ok(())
}

/// Create a detached git worktree of `source` at `dest` on a new branch.
pub fn create_worktree(source: &Path, dest: &Path, branch: &str) -> Result<(), StoreError> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }

    // `git worktree add -b` refuses if the destination already exists, so
    // surface a clear error instead of letting git fail cryptically.
    if dest.exists() {
        return Err(StoreError::Other(format!(
            "worktree destination already exists: {}",
            dest.display()
        )));
    }

    let output = Command::new("git")
        .arg("worktree")
        .arg("add")
        .arg("-b")
        .arg(branch)
        .arg(dest)
        .current_dir(source)
        .output()
        .map_err(|e| StoreError::Other(format!("failed to spawn git: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(StoreError::Other(format!(
            "git worktree add exited {}: {}",
            output.status,
            stderr.trim()
        )));
    }
    Ok(())
}

/// Copy every attachment belonging to `feedback` into `inputs_dir` so the
/// spawned `claude` process can Read them by absolute path from inside the
/// worktree. Returns a human-readable list of the copied paths for the
/// prompt. Missing attachments are skipped — it's a best-effort snapshot.
pub fn stage_attachments(
    store: &Store,
    feedback: &FeedbackRecord,
    inputs_dir: &Path,
) -> Result<Vec<StagedAttachment>, StoreError> {
    fs::create_dir_all(inputs_dir)?;
    let mut staged = Vec::new();

    let mut copy = |role: &str, filename: &str| -> Result<(), StoreError> {
        if let Some(bytes) = store.read_attachment(&feedback.id, filename)? {
            let dest = inputs_dir.join(filename);
            fs::write(&dest, &bytes)?;
            staged.push(StagedAttachment {
                role: role.to_string(),
                path: dest,
            });
        }
        Ok(())
    };

    if let Some(name) = feedback.screenshot_filename.as_deref() {
        copy("canvas screenshot", name)?;
    }
    for name in &feedback.pasted_images {
        copy("pasted image", name)?;
    }
    if let Some(name) = feedback.voice_filename.as_deref() {
        copy("voice recording", name)?;
    }

    // Annotations ride along as JSON so claude can inspect shape coords.
    if !feedback.annotations.is_empty() {
        let dest = inputs_dir.join("annotations.json");
        let body = serde_json::to_string_pretty(&feedback.annotations)?;
        fs::write(&dest, body)?;
        staged.push(StagedAttachment {
            role: "annotations".into(),
            path: dest,
        });
    }

    Ok(staged)
}

pub struct StagedAttachment {
    pub role: String,
    pub path: PathBuf,
}

/// Construct the prompt handed to Claude Code. Kept public so tests (and
/// future tooling) can assert it contains the right invariants.
/// Returns the tailored guidance block for a given iteration number. The
/// meta-app starts empty: iteration 1 is the user describing the app they
/// actually want (via canvas + text + voice), and the agent has wide latitude
/// to rearchitect the codebase. As the iteration number grows, the agent's
/// freedom narrows — by ~iteration 10 the default is minor, surgical fixes
/// unless the user explicitly asks for a structural change.
///
/// Regardless of iteration, the four product invariants (Feedback Overlay,
/// Canvas / drawing board, Inbox, Sandbox pipeline) MUST survive every pass.
pub fn iteration_guidance(iteration: u32) -> String {
    let n = iteration.max(1);
    let (phase, latitude) = match n {
        1..=3 => (
            "Bootstrap phase",
            "You have wide latitude to make drastic architectural and source-code changes. \
             Treat the existing NoIDE shell as scaffolding: rip out, rename, restructure, and replace code \
             as needed to realise the app the user has described in the canvas, voice, and text. \
             Follow the user's described app (ERP, IDE, fitness tracker, whatever they drew) as faithfully as you can.",
        ),
        4..=6 => (
            "Shaping phase",
            "Significant changes are still welcome when they move the app toward the user's described vision, \
             but prefer cohesive feature additions over wholesale rewrites. Refactor when it clearly serves the feedback.",
        ),
        7..=9 => (
            "Consolidation phase",
            "Prefer targeted changes that extend or refine existing features. Only restructure code if the feedback \
             explicitly calls for it or the current shape blocks the change.",
        ),
        _ => (
            "Maturation phase",
            "Default to minor, surgical changes. Do NOT refactor unrelated code, rename modules, or restructure the app \
             unless the user explicitly asks for it in this feedback.",
        ),
    };

    format!(
        r#"# Iteration {n} — {phase}

This NoIDE instance is a self-evolving meta-app. Each approved sandbox job is one iteration in the life of the app the user is building on top of the NoIDE shell.

**Latitude for this iteration:** {latitude}

## Invariants you MUST preserve on every iteration, no matter what

Whatever the app becomes, the shell must keep these four surfaces reachable and functional:

1. **Feedback Overlay** — reachable from every screen, every mode. The user must always be able to open the feedback panel and submit new feedback about the page they are on.
2. **Canvas overlay on every page** — the Canvas is NOT a standalone tab or dedicated route. It is an overlay the user can open on top of *any* page of the app to draw / annotate / sketch feedback about *that specific page*. Every route must support opening the Canvas on top of it; the feedback submission records which route the drawing was made on. A design that only lets the user draw on a single "Canvas tab" is wrong — the whole point is per-page visual feedback.
3. **Inbox** — the list/overview of submitted feedback must remain visible and navigable, and each entry must preserve the page/route it was submitted from.
4. **Sandbox pipeline** — the feedback → sandbox-job state machine (and the Advance / Retry / Reject / Run affordances) must keep working end-to-end so the *next* iteration can happen.

If your change would break any of these four surfaces in the resulting app, it is wrong — redesign the change to preserve them. These invariants are load-bearing; they are what makes iteration N+1 possible.

## Context hygiene — update docs and agents alongside the code

Because future iterations rely on the repo's own documentation for context, any non-trivial change to the app MUST also update:

- `CLAUDE.md` — reflect the new architecture, stack, commands, domain model. Remove stale sections rather than layering on top.
- `.claude/rules/` — update conventions that no longer match the code (or add new ones). Delete rules for layers that no longer exist.
- `.claude/agents/*` — if an agent's description, responsibilities, or tools no longer match the current codebase, update its frontmatter and body. If a whole agent is obsolete, delete it; if the app now needs a new specialist, add one.
- `.claude/skills/*` (if present) — same treatment: keep them accurate or remove them.

The next iteration's agent will read these files first. Leaving them stale is the single biggest way to sabotage iteration N+1.

## Keep the iteration runnable — `scripts/run-iteration.sh`

The reviewer UI has a **Run** button that launches the app built in this iteration's worktree. It invokes `scripts/run-iteration.sh` at the worktree root if present, otherwise falls back to `cargo tauri dev` in `app/src-tauri`.

If you rewrite the stack (e.g. move off Tauri/Leptos) you MUST create or update `scripts/run-iteration.sh` so the Run button still works. The script should:

- Start the current app in the foreground (the runner streams its stdout/stderr into a log file).
- Respect `NOIDE_WORKSPACE_ROOT` if the app stores any state — the runner sets that env var to a per-iteration workspace directory so runs stay isolated from the host NoIDE.
- Exit non-zero on startup failure so the reviewer sees a useful error in the sandbox notes.

If you kept the default stack, you can skip the script and rely on the `cargo tauri dev` fallback.
"#,
    )
}

pub fn build_implementation_prompt(
    feedback: &FeedbackRecord,
    job: &SandboxJobRecord,
    attachments: &[StagedAttachment],
    log_file: &Path,
) -> String {
    let iteration = if job.iteration == 0 { 1 } else { job.iteration };
    let guidance = iteration_guidance(iteration);
    let work_step_4 = if iteration <= 3 {
        "4. Make the change the user described. On this iteration you are allowed — and expected — to restructure the codebase to fit the app the user drew. Touch as much as you need; just keep the four invariants above intact."
    } else if iteration <= 9 {
        "4. Make a change that clearly resolves the feedback and moves the app toward the user's described vision. Refactor when it serves the goal; don't refactor for its own sake."
    } else {
        "4. Make the minimal, focused change that actually resolves the feedback. Do not refactor unrelated code unless the user explicitly asks for it."
    };
    let feedback_type = format!("{:?}", feedback.feedback_type);
    let route = if feedback.page_route.is_empty() {
        "/".into()
    } else {
        feedback.page_route.clone()
    };
    let voice_line = feedback
        .voice_transcript
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(|t| format!("\n\n## Voice transcript\n\n{t}"))
        .unwrap_or_default();

    let attachments_section = if attachments.is_empty() {
        String::new()
    } else {
        let mut s = String::from("\n\n## Attachments (read these with the Read tool before planning)\n");
        for a in attachments {
            s.push_str(&format!("- **{}** — `{}`\n", a.role, a.path.display()));
        }
        s
    };

    format!(
        r#"You are running inside a sandboxed git worktree of the NoIDE project. A user submitted feedback through the in-app feedback panel and a reviewer pressed "Advance" on the resulting sandbox job. Your job: implement the change.

{guidance}

# Sandbox job

- Job ID: `{job_id}`
- Branch: `{branch}`
- Iteration: `{iteration}`
- Title: {title}
- Feedback type: {feedback_type}
- Submitted from route: {route}

# What the user said

{feedback_text}{voice_line}{attachments_section}

# How to work

1. Read `CLAUDE.md` and skim the relevant files under `app/` to orient yourself. Also skim `.claude/rules/` and `.claude/agents/` so you know what docs you will be expected to update.
2. Read every file listed under Attachments above — the screenshot is often the clearest statement of intent of the app the user is building.
3. When the work calls for a specialist, delegate via the Agent tool to one of the project agents defined in `.claude/agents/` (use whichever agents exist in this iteration of the repo — names may have changed).
{work_step_4}
5. If the app's architecture, stack, domain model, or command surface changed materially: update `CLAUDE.md`, the relevant files under `.claude/rules/`, and the affected `.claude/agents/*.md` (and `.claude/skills/*` if present) so the next iteration's agent starts with accurate context. Stale docs are treated as a bug.
6. Run the appropriate checks before finishing. The exact commands depend on the current stack — read `CLAUDE.md` for the build contract. For today's Rust + Leptos + Tauri shell the defaults are:
   - Backend: `cargo check -p noide_desktop`
   - UI: `cargo check -p noide_ui --target wasm32-unknown-unknown`
   If you rewrote the stack, run the equivalent checks for the new stack and update `CLAUDE.md` to document them.
7. Commit your work with `git add -A && git commit` so the reviewer can diff the branch. Use a conventional-commit subject line like `feat(ui): …` or `fix(sandbox): …`.
8. Print a short summary (5-10 lines) of what you changed, which files were touched, and — if invariants were at risk — how you preserved Feedback Overlay / Canvas / Inbox / Sandbox. Keep it focused — the reviewer reads this first.

# Safety

- You are on branch `{branch}` in an isolated worktree. Do not `git push`, do not switch branches, do not touch the main branch.
- You are running with `--permission-mode acceptEdits`: file edits inside this worktree are auto-approved, but genuinely risky operations still need confirmation.
- If a dependency is missing or the task is impossible in this environment, say so plainly and exit — do not fake success.
- Your full transcript is being captured at `{log_file}` for reviewer audit.
"#,
        guidance = guidance,
        job_id = job.id,
        branch = branch_name(&job.id),
        iteration = iteration,
        title = job.title,
        feedback_text = feedback.feedback_text,
        log_file = log_file.display(),
        work_step_4 = work_step_4,
    )
}

/// Returned synchronously from `prepare_run` so the caller can persist the
/// new paths onto the job record before the async run starts.
pub struct PreparedRun {
    pub worktree: PathBuf,
    pub log_file: PathBuf,
    pub prompt_file: PathBuf,
    pub metadata_file: PathBuf,
    pub inputs_dir: PathBuf,
    pub branch: String,
    pub source_repo: PathBuf,
    pub prompt: String,
}

/// Build the worktree, stage attachments, and write the prompt + metadata
/// files. Does NOT launch claude — that happens in `launch_claude`, so the
/// caller has the chance to persist artifact paths onto the job record
/// first.
pub fn prepare_run(
    store: &Store,
    job: &SandboxJobRecord,
    feedback: &FeedbackRecord,
) -> Result<PreparedRun, StoreError> {
    // Lazily allocate an iteration number for this job. We work on a local
    // copy so the caller's `&SandboxJobRecord` signature stays intact; the
    // allocated iteration is persisted back onto the stored record so the
    // UI (and any retry) sees a stable value.
    let mut job = job.clone();
    if job.iteration == 0 {
        let n = store.allocate_iteration()?;
        job.iteration = n;
        store.save_sandbox_job(&job)?;
    }
    let job = &job;
    let source = resolve_source_repo().ok_or_else(|| {
        StoreError::Other(
            "could not locate NoIDE source repo — set NOIDE_SOURCE_REPO or run from within the repo"
                .to_string(),
        )
    })?;

    let root = store.layout().root().to_path_buf();
    let job_dir = job_workspace_dir(&root, &job.id);
    fs::create_dir_all(&job_dir)?;

    let worktree = worktree_path(&root, &job.id);
    let inputs_dir = inputs_path(&root, &job.id);
    let log_file = log_path(&root, &job.id);
    let prompt_file = prompt_path(&root, &job.id);
    let metadata_file = metadata_path(&root, &job.id);
    let branch = branch_name(&job.id);

    create_worktree(&source, &worktree, &branch)?;
    let attachments = stage_attachments(store, feedback, &inputs_dir)?;

    let prompt = build_implementation_prompt(feedback, job, &attachments, &log_file);
    fs::write(&prompt_file, &prompt)?;

    let metadata = serde_json::json!({
        "job_id": job.id,
        "feedback_id": feedback.id,
        "iteration": job.iteration,
        "branch": branch,
        "worktree": worktree.display().to_string(),
        "source_repo": source.display().to_string(),
        "log_file": log_file.display().to_string(),
        "prompt_file": prompt_file.display().to_string(),
        "attachments": attachments.iter().map(|a| serde_json::json!({
            "role": a.role,
            "path": a.path.display().to_string(),
        })).collect::<Vec<_>>(),
        "permission_mode": "acceptEdits",
        "started_at_unix_ms": crate::types::current_time_unix_ms(),
    });
    fs::write(&metadata_file, serde_json::to_string_pretty(&metadata)?)?;

    Ok(PreparedRun {
        worktree,
        log_file,
        prompt_file,
        metadata_file,
        inputs_dir,
        branch,
        source_repo: source,
        prompt,
    })
}

/// Spawn `claude -p …` in `worktree`, stream its output to `log_file`, and
/// transition the job when it finishes. Returns immediately; all I/O
/// happens on a dedicated OS thread.
pub fn launch_claude(store: Store, job_id: String, prepared: PreparedRun) {
    std::thread::spawn(move || {
        let engine = SandboxEngine::new(&store);

        let log_handle = match fs::File::create(&prepared.log_file) {
            Ok(f) => f,
            Err(e) => {
                let _ = engine.append_note(
                    &job_id,
                    &format!("failed to open log {}: {e}", prepared.log_file.display()),
                );
                let _ = engine.force_status(&job_id, SandboxJobStatus::Failed);
                return;
            }
        };
        let log_for_err = match log_handle.try_clone() {
            Ok(f) => f,
            Err(e) => {
                let _ = engine
                    .append_note(&job_id, &format!("failed to clone log handle: {e}"));
                let _ = engine.force_status(&job_id, SandboxJobStatus::Failed);
                return;
            }
        };

        let _ = engine.append_note(
            &job_id,
            &format!(
                "claude code starting (permission-mode=acceptEdits, auth=subscription) in worktree {} — streaming to {}",
                prepared.worktree.display(),
                prepared.log_file.display(),
            ),
        );

        // Force the Claude Max subscription auth path by scrubbing
        // `ANTHROPIC_API_KEY` from the child environment. The CLI prefers the
        // API key whenever it's present, which silently breaks users whose
        // key has no balance but who are separately logged in via
        // `claude login`. Scrubbing `ANTHROPIC_AUTH_TOKEN` as well for the
        // same reason (internal Anthropic tooling override).
        let status = Command::new("claude")
            .arg("-p")
            .arg(&prepared.prompt)
            .arg("--permission-mode")
            .arg("acceptEdits")
            .current_dir(&prepared.worktree)
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("ANTHROPIC_AUTH_TOKEN")
            .stdin(Stdio::null())
            .stdout(Stdio::from(log_handle))
            .stderr(Stdio::from(log_for_err))
            .status();

        match status {
            Ok(s) if s.success() => {
                let _ = engine.append_note(&job_id, "claude code completed successfully");
                let _ = engine.force_status(&job_id, SandboxJobStatus::BuildReady);
            }
            Ok(s) => {
                let _ = engine.append_note(
                    &job_id,
                    &format!(
                        "claude exited with status {s} — see log {}",
                        prepared.log_file.display()
                    ),
                );
                let _ = engine.force_status(&job_id, SandboxJobStatus::Failed);
            }
            Err(e) => {
                let _ = engine.append_note(
                    &job_id,
                    &format!(
                        "failed to launch claude ({e}) — ensure the `claude` CLI is installed and in PATH"
                    ),
                );
                let _ = engine.force_status(&job_id, SandboxJobStatus::Failed);
            }
        }
    });
}

pub fn iteration_run_log_path(workspace_root: &Path, job_id: &str) -> PathBuf {
    job_workspace_dir(workspace_root, job_id).join(RUN_LOG_FILE)
}

pub fn iteration_run_workspace(workspace_root: &Path, job_id: &str) -> PathBuf {
    job_workspace_dir(workspace_root, job_id).join(RUN_WORKSPACE_DIR)
}

/// What command to execute when the reviewer clicks "Run" on a completed
/// iteration. We prefer `scripts/run-iteration.sh` at the worktree root so
/// the agent can rewrite the stack freely and still expose a stable entry
/// point. If the script isn't there, fall back to `cargo tauri dev` against
/// the default NoIDE shell location (`app/src-tauri`).
pub struct ResolvedRunCommand {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub source: &'static str,
}

pub fn resolve_run_command(worktree: &Path) -> ResolvedRunCommand {
    let script = worktree.join(DEFAULT_RUN_SCRIPT);
    if script.exists() {
        return ResolvedRunCommand {
            program: "bash".into(),
            args: vec![script.display().to_string()],
            cwd: worktree.to_path_buf(),
            source: "scripts/run-iteration.sh",
        };
    }
    ResolvedRunCommand {
        program: "cargo".into(),
        args: vec!["tauri".into(), "dev".into()],
        cwd: worktree.join("app").join("src-tauri"),
        source: "cargo tauri dev (fallback)",
    }
}

/// Spawn the iteration's app from its sandbox worktree and stream output to
/// `iteration-run.log` under the job workspace. Fire-and-forget: the call
/// returns as soon as the child is handed off to a dedicated thread; the
/// status the user sees in the UI reflects the sandbox state machine, not
/// the run process itself. The child gets its own `NOIDE_WORKSPACE_ROOT`
/// pointed at the per-job `run_workspace/` dir so it can't clobber the host
/// NoIDE's feedback / sandbox data.
pub fn launch_iteration_run(store: Store, job_id: String) {
    std::thread::spawn(move || {
        let engine = SandboxEngine::new(&store);

        let Some(job) = store.load_sandbox_job(&job_id).ok().flatten() else {
            let _ = engine.append_note(&job_id, "run requested but sandbox job record is missing");
            return;
        };
        let Some(worktree_str) = job.worktree_path.clone() else {
            let _ = engine.append_note(
                &job_id,
                "run requested but this job has no worktree yet — advance it first",
            );
            return;
        };
        let worktree = PathBuf::from(&worktree_str);
        if !worktree.exists() {
            let _ = engine.append_note(
                &job_id,
                &format!("run requested but worktree is gone: {worktree_str}"),
            );
            return;
        }

        let root = store.layout().root().to_path_buf();
        let log_path = iteration_run_log_path(&root, &job_id);
        let run_workspace = iteration_run_workspace(&root, &job_id);
        let _ = fs::create_dir_all(&run_workspace);

        let log_handle = match fs::File::create(&log_path) {
            Ok(f) => f,
            Err(e) => {
                let _ = engine.append_note(
                    &job_id,
                    &format!("failed to open run log {}: {e}", log_path.display()),
                );
                return;
            }
        };
        let log_for_err = match log_handle.try_clone() {
            Ok(f) => f,
            Err(e) => {
                let _ = engine.append_note(
                    &job_id,
                    &format!("failed to clone run log handle: {e}"),
                );
                return;
            }
        };

        let cmd = resolve_run_command(&worktree);
        let _ = engine.append_note(
            &job_id,
            &format!(
                "launching iteration run via {} ({} {}) in {} — log {}",
                cmd.source,
                cmd.program,
                cmd.args.join(" "),
                cmd.cwd.display(),
                log_path.display(),
            ),
        );

        let status = Command::new(&cmd.program)
            .args(&cmd.args)
            .current_dir(&cmd.cwd)
            .env("NOIDE_WORKSPACE_ROOT", &run_workspace)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log_handle))
            .stderr(Stdio::from(log_for_err))
            .status();

        match status {
            Ok(s) if s.success() => {
                let _ = engine.append_note(
                    &job_id,
                    &format!("iteration run exited cleanly — see {}", log_path.display()),
                );
            }
            Ok(s) => {
                let _ = engine.append_note(
                    &job_id,
                    &format!(
                        "iteration run exited with status {s} — see {}",
                        log_path.display()
                    ),
                );
            }
            Err(e) => {
                let _ = engine.append_note(
                    &job_id,
                    &format!(
                        "failed to launch iteration run ({e}) — ensure `{}` is installed and in PATH",
                        cmd.program
                    ),
                );
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FeedbackStatus, FeedbackType};

    fn mk_feedback() -> FeedbackRecord {
        FeedbackRecord {
            id: "fb-42".into(),
            feedback_type: FeedbackType::Bug,
            status: FeedbackStatus::InSandbox,
            page_route: "/".into(),
            feedback_text: "The save button sometimes doesn't respond".into(),
            annotations: vec![serde_json::json!({"type": "rect"})],
            pasted_images: vec!["paste-0.png".into()],
            screenshot_filename: Some("canvas.png".into()),
            voice_filename: None,
            voice_transcript: Some("the button feels laggy".into()),
            window_width: 1440,
            window_height: 900,
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
            sandbox_job_id: Some("job-42".into()),
        }
    }

    fn mk_job() -> SandboxJobRecord {
        SandboxJobRecord {
            id: "job-42".into(),
            feedback_id: "fb-42".into(),
            title: "Save button is laggy".into(),
            summary: "…".into(),
            status: SandboxJobStatus::Pending,
            notes: vec![],
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
            worktree_path: None,
            branch_name: None,
            log_path: None,
            source_repo: None,
            iteration: 0,
        }
    }

    #[test]
    fn prompt_contains_job_feedback_and_attachment_paths() {
        let attachments = vec![
            StagedAttachment {
                role: "canvas screenshot".into(),
                path: PathBuf::from("/tmp/job-42/inputs/canvas.png"),
            },
            StagedAttachment {
                role: "annotations".into(),
                path: PathBuf::from("/tmp/job-42/inputs/annotations.json"),
            },
        ];
        let prompt = build_implementation_prompt(
            &mk_feedback(),
            &mk_job(),
            &attachments,
            Path::new("/tmp/job-42/claude.log"),
        );

        assert!(prompt.contains("job-42"));
        assert!(prompt.contains("sandbox/job-42"));
        assert!(prompt.contains("Save button is laggy"));
        assert!(prompt.contains("The save button sometimes doesn't respond"));
        assert!(prompt.contains("the button feels laggy"));
        assert!(prompt.contains("canvas screenshot"));
        assert!(prompt.contains("/tmp/job-42/inputs/canvas.png"));
        assert!(prompt.contains("/tmp/job-42/inputs/annotations.json"));
        assert!(prompt.contains("/tmp/job-42/claude.log"));
        assert!(prompt.contains(".claude/agents/"));
        assert!(prompt.contains("acceptEdits"));
    }

    #[test]
    fn prompt_omits_attachment_block_when_empty() {
        let prompt = build_implementation_prompt(
            &mk_feedback(),
            &mk_job(),
            &[],
            Path::new("/tmp/claude.log"),
        );
        assert!(!prompt.contains("## Attachments"));
    }

    #[test]
    fn iteration_guidance_shifts_with_iteration() {
        let early = iteration_guidance(1);
        assert!(early.contains("Iteration 1"));
        assert!(early.contains("Bootstrap"));
        assert!(early.contains("wide latitude"));

        let mid = iteration_guidance(5);
        assert!(mid.contains("Iteration 5"));
        assert!(mid.contains("Shaping"));

        let late = iteration_guidance(12);
        assert!(late.contains("Iteration 12"));
        assert!(late.contains("Maturation"));
        assert!(late.contains("minor, surgical"));

        // Invariants section appears in every phase.
        for g in [&early, &mid, &late] {
            assert!(g.contains("Feedback Overlay"));
            assert!(g.contains("Canvas"));
            assert!(g.contains("Inbox"));
            assert!(g.contains("Sandbox"));
            assert!(g.contains("CLAUDE.md"));
            assert!(g.contains(".claude/agents"));
        }
    }

    #[test]
    fn prompt_defaults_unallocated_iteration_to_one() {
        let prompt = build_implementation_prompt(
            &mk_feedback(),
            &mk_job(), // iteration: 0
            &[],
            Path::new("/tmp/claude.log"),
        );
        assert!(prompt.contains("Iteration: `1`"));
        assert!(prompt.contains("Bootstrap phase"));
    }

    #[test]
    fn prompt_uses_job_iteration_when_set() {
        let mut job = mk_job();
        job.iteration = 10;
        let prompt = build_implementation_prompt(
            &mk_feedback(),
            &job,
            &[],
            Path::new("/tmp/claude.log"),
        );
        assert!(prompt.contains("Iteration: `10`"));
        assert!(prompt.contains("Maturation phase"));
        assert!(prompt.contains("minimal, focused change"));
    }

    #[test]
    fn resolve_run_command_prefers_iteration_script_when_present() {
        let temp = tempfile::tempdir().unwrap();
        let worktree = temp.path().to_path_buf();
        fs::create_dir_all(worktree.join("scripts")).unwrap();

        // Without the script, we fall back to cargo tauri dev.
        let fallback = resolve_run_command(&worktree);
        assert_eq!(fallback.program, "cargo");
        assert_eq!(fallback.args, vec!["tauri".to_string(), "dev".to_string()]);
        assert!(fallback.cwd.ends_with("app/src-tauri"));

        // With the script present, we prefer it.
        fs::write(worktree.join("scripts").join("run-iteration.sh"), b"#!/bin/sh\n").unwrap();
        let chosen = resolve_run_command(&worktree);
        assert_eq!(chosen.program, "bash");
        assert!(chosen.args[0].ends_with("scripts/run-iteration.sh"));
        assert_eq!(chosen.cwd, worktree);
    }

    #[test]
    fn iteration_run_log_path_nests_under_job_workspace() {
        let root = PathBuf::from("/tmp/ws");
        let p = iteration_run_log_path(&root, "job-7");
        assert!(p.ends_with("sandbox_workspaces/job-7/iteration-run.log"));
    }

    #[test]
    fn branch_name_uses_sandbox_prefix() {
        assert_eq!(branch_name("job-123"), "sandbox/job-123");
    }

    #[test]
    fn layout_paths_stay_under_workspace() {
        let root = PathBuf::from("/tmp/ws");
        let worktree = worktree_path(&root, "job-9");
        assert!(worktree.starts_with(&root));
        assert!(worktree.ends_with("sandbox_workspaces/job-9/worktree"));

        let log = log_path(&root, "job-9");
        assert!(log.ends_with("sandbox_workspaces/job-9/claude.log"));

        let inputs = inputs_path(&root, "job-9");
        assert!(inputs.ends_with("sandbox_workspaces/job-9/inputs"));
    }

    #[test]
    fn resolve_source_repo_finds_noide_checkout() {
        let resolved = resolve_source_repo();
        assert!(resolved.is_some(), "should find the NoIDE source repo under the test harness");
        let p = resolved.unwrap();
        assert!(p.join(".claude").join("agents").exists());
        assert!(p.join(".git").exists());
    }

    #[test]
    fn stage_attachments_copies_every_role() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::new(temp.path().to_path_buf());
        store.init_workspace().unwrap();
        let fb = mk_feedback();
        // Put the referenced attachments into the store.
        store.save_attachment(&fb.id, "canvas.png", b"png-bytes").unwrap();
        store.save_attachment(&fb.id, "paste-0.png", b"paste-bytes").unwrap();

        let inputs = temp.path().join("job-42").join("inputs");
        let staged = stage_attachments(&store, &fb, &inputs).unwrap();

        // canvas + paste-0 + annotations.json — voice was None, so skipped.
        assert_eq!(staged.len(), 3);
        assert!(inputs.join("canvas.png").exists());
        assert!(inputs.join("paste-0.png").exists());
        assert!(inputs.join("annotations.json").exists());
    }
}
