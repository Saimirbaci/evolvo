//! Runs Claude Code non-interactively inside an isolated git worktree
//! forked from the Evolvo source repo. This is the bridge between a reviewer
//! pressing "Advance" on a lineage job and actual code being written.
//!
//! Safety posture:
//! - The worktree lives on its own branch (`lineage/<job-id>`) so Claude
//!   can never touch the main branch or the primary checkout.
//! - Claude is launched with `--dangerously-skip-permissions`. The lineage
//!   worktree + throwaway branch provide the safety envelope; inside that
//!   envelope the agent needs to actually run `cargo`, `git`, `trunk`,
//!   `bash scripts/run-iteration.sh`, etc. without the user standing over
//!   it hitting "approve". Every file the agent writes lives on
//!   `lineage/<job-id>` and every command runs in the worktree dir, so the
//!   blast radius is bounded even with full tool access.
//! - All stdout + stderr is streamed to `claude.log` under the job's lineage
//!   workspace directory, and every state transition is appended to the job
//!   record's notes for observability.
//! - If `claude` or `git` is missing the job transitions to `Failed` with a
//!   note explaining how to fix the environment.
//!
//! Source repo resolution order:
//! 1. `EVOLVO_SOURCE_REPO` env var.
//! 2. Walk up from `CARGO_MANIFEST_DIR` (compile-time) for a directory that
//!    has both `.git/` and `.claude/agents/`.
//! 3. Walk up from the process CWD using the same check.

use std::fs;
#[cfg(unix)]
use std::os::unix::fs as unix_fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::agent::{backend_for, AgentBackend};
use crate::lineage::LineageEngine;
use crate::store::{Store, StoreError};
use crate::types::{AgentKind, FeedbackRecord, LineageJobRecord, LineageJobStatus};

const SANDBOX_WORKSPACES_DIR: &str = "lineage_workspaces";
const WORKTREE_DIR: &str = "worktree";
const INPUTS_DIR: &str = "inputs";
/// Default agent log filename. Retained for compatibility with tests /
/// callers that refer to the historical Claude-only path; the runtime path
/// is computed from `AgentBackend::log_filename`.
const LOG_FILE: &str = "claude.log";
const PROMPT_FILE: &str = "prompt.md";
const METADATA_FILE: &str = "run.json";
const RUN_LOG_FILE: &str = "iteration-run.log";
const RUN_WORKSPACE_DIR: &str = "run_workspace";
const DEFAULT_RUN_SCRIPT: &str = "scripts/run-iteration.sh";

/// Base dev-server port used by the host Evolvo (iteration 0). Each iteration
/// bumps this by its iteration number so concurrent iteration runs don't
/// collide on the same port — iteration 1 lives on `BASE + 1`, iteration 2 on
/// `BASE + 2`, and so on. Kept in sync with `app/src-tauri/tauri.conf.json`'s
/// `devUrl` and `app/ui/Trunk.toml`'s `serve.port`.
pub const BASE_DEV_PORT: u16 = 1530;

/// Port the iteration's dev server should bind to. `iteration = 0` is the
/// host Evolvo itself; real lineage iterations start at 1. Capped at ~65500
/// defensively, though in practice nobody reaches 64k iterations.
pub fn iteration_port(iteration: u32) -> u16 {
    let shifted = (BASE_DEV_PORT as u32).saturating_add(iteration);
    shifted.min(65500) as u16
}

/// Locate the Evolvo source repo that should be forked into the lineage.
pub fn resolve_source_repo() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("EVOLVO_SOURCE_REPO") {
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

pub fn lineage_workspaces_root(workspace_root: &Path) -> PathBuf {
    workspace_root.join(SANDBOX_WORKSPACES_DIR)
}

pub fn job_workspace_dir(workspace_root: &Path, job_id: &str) -> PathBuf {
    lineage_workspaces_root(workspace_root).join(job_id)
}

pub fn worktree_path(workspace_root: &Path, job_id: &str) -> PathBuf {
    job_workspace_dir(workspace_root, job_id).join(WORKTREE_DIR)
}

pub fn inputs_path(workspace_root: &Path, job_id: &str) -> PathBuf {
    job_workspace_dir(workspace_root, job_id).join(INPUTS_DIR)
}

/// Legacy Claude-only log path. Kept for any external caller that still
/// expects the `claude.log` filename; new code should prefer
/// [`agent_log_path`] so each backend gets its own transcript filename.
pub fn log_path(workspace_root: &Path, job_id: &str) -> PathBuf {
    job_workspace_dir(workspace_root, job_id).join(LOG_FILE)
}

/// Per-backend log path. The filename comes from the backend itself
/// (`claude.log`, `codex.log`, `gemini.log`, `forge.log`) so a single
/// lineage workspace can host retries across different agents without
/// clobbering the transcript.
pub fn agent_log_path(workspace_root: &Path, job_id: &str, agent: AgentKind) -> PathBuf {
    let filename = backend_for(agent).log_filename();
    job_workspace_dir(workspace_root, job_id).join(filename)
}

/// Materialise the worktree-root project guide files an agent expects
/// (`AGENTS.md` for Codex/Forge, `GEMINI.md` for Gemini, etc.) by
/// symlinking them to the repo's existing `CLAUDE.md`. Best-effort: if a
/// file with the same name already exists we leave it alone, and if the
/// symlink syscall isn't available we fall back to `fs::copy`.
///
/// This keeps the four agents discovering the same canonical project
/// context without forcing the repo to carry four duplicate copies.
fn materialise_context_files(worktree: &Path, backend: &dyn AgentBackend) -> Vec<PathBuf> {
    let mut created = Vec::new();
    let source = worktree.join("CLAUDE.md");
    if !source.exists() {
        return created;
    }
    for rel in backend.context_files() {
        // Only materialise top-level file aliases — skip directories like
        // `.claude/agents` and other already-present files.
        if rel.contains('/') {
            continue;
        }
        if *rel == "CLAUDE.md" {
            continue; // already the source
        }
        let dest = worktree.join(rel);
        if dest.exists() {
            continue;
        }
        #[cfg(unix)]
        let linked = unix_fs::symlink(&source, &dest).is_ok();
        #[cfg(not(unix))]
        let linked = false;
        if linked {
            created.push(dest);
            continue;
        }
        if fs::copy(&source, &dest).is_ok() {
            created.push(dest);
        }
    }
    created
}

pub fn prompt_path(workspace_root: &Path, job_id: &str) -> PathBuf {
    job_workspace_dir(workspace_root, job_id).join(PROMPT_FILE)
}

pub fn metadata_path(workspace_root: &Path, job_id: &str) -> PathBuf {
    job_workspace_dir(workspace_root, job_id).join(METADATA_FILE)
}

pub fn branch_name(job_id: &str) -> String {
    format!("lineage/{job_id}")
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

/// Rewrite the dev-server port in a worktree so iteration N listens on
/// `BASE_DEV_PORT + N` instead of the base port. Best-effort: patches every
/// file it recognises and ignores missing ones (the agent may have rewritten
/// the stack, in which case they're responsible for handling the port via
/// `scripts/run-iteration.sh` + the `EVOLVO_ITERATION_PORT` env var).
///
/// Returns the list of files that were actually modified so callers can log
/// a useful breadcrumb.
pub fn rewrite_iteration_port(worktree: &Path, port: u16) -> Result<Vec<PathBuf>, StoreError> {
    let base_str = BASE_DEV_PORT.to_string();
    let port_str = port.to_string();
    if base_str == port_str {
        return Ok(Vec::new());
    }

    let mut changed = Vec::new();
    let targets: &[&str] = &[
        "app/src-tauri/tauri.conf.json",
        "app/ui/Trunk.toml",
        "app/ui/scripts/trunk-dev.sh",
    ];

    for rel in targets {
        let path = worktree.join(rel);
        if !path.exists() {
            continue;
        }
        let Ok(body) = fs::read_to_string(&path) else {
            continue;
        };
        if !body.contains(&base_str) {
            continue;
        }
        // Replace any bare occurrence of the base port. The substrings we're
        // replacing (":1530", "port = 1530", "--port 1530") are distinctive
        // enough that a naive replace_all is safe — the base port number
        // doesn't collide with anything else in these files.
        let new_body = body.replace(&base_str, &port_str);
        if new_body != body {
            fs::write(&path, new_body)?;
            changed.push(path);
        }
    }
    Ok(changed)
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

/// Embedded fallbacks so the binary still works when the prompts dir is
/// missing. Overrides are resolved at runtime from (in order):
///   1. `$EVOLVO_PROMPTS_DIR`
///   2. `<source_repo>/prompts/`
///   3. the compiled-in string below.
/// This lets the user edit the on-disk prompt files and see the change on the
/// next lineage run without rebuilding the host binary.
const EMBEDDED_ITERATION_GUIDANCE: &str = include_str!("../../../prompts/iteration_guidance.md");
const EMBEDDED_IMPLEMENTATION: &str = include_str!("../../../prompts/implementation.md");
const EMBEDDED_NEW_APP_BANNER: &str = include_str!("../../../prompts/new_app_banner.md");
const EMBEDDED_PHASE_BOOTSTRAP: &str = include_str!("../../../prompts/phases/bootstrap.md");
const EMBEDDED_PHASE_SHAPING: &str = include_str!("../../../prompts/phases/shaping.md");
const EMBEDDED_PHASE_CONSOLIDATION: &str =
    include_str!("../../../prompts/phases/consolidation.md");
const EMBEDDED_PHASE_MATURATION: &str = include_str!("../../../prompts/phases/maturation.md");
const EMBEDDED_STEP_NEW_APP: &str = include_str!("../../../prompts/work_steps/new_app.md");
const EMBEDDED_STEP_BOOTSTRAP: &str = include_str!("../../../prompts/work_steps/bootstrap.md");
const EMBEDDED_STEP_SHAPING: &str = include_str!("../../../prompts/work_steps/shaping.md");
const EMBEDDED_STEP_MATURATION: &str = include_str!("../../../prompts/work_steps/maturation.md");
const EMBEDDED_STAGE_BACKEND_PLAN: &str = include_str!("../../../prompts/stages/backend_plan.md");
const EMBEDDED_STAGE_BACKEND_IMPL: &str = include_str!("../../../prompts/stages/backend_impl.md");
const EMBEDDED_STAGE_FRONTEND_PLAN: &str = include_str!("../../../prompts/stages/frontend_plan.md");
const EMBEDDED_STAGE_FRONTEND_IMPL: &str = include_str!("../../../prompts/stages/frontend_impl.md");
const EMBEDDED_STAGE_E2E_PLAN: &str = include_str!("../../../prompts/stages/e2e_plan.md");
const EMBEDDED_STAGE_E2E_IMPL: &str = include_str!("../../../prompts/stages/e2e_impl.md");
const EMBEDDED_STAGE_FINAL_REVIEW: &str = include_str!("../../../prompts/stages/final_review.md");

fn prompts_override_dir() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("EVOLVO_PROMPTS_DIR") {
        let pb = PathBuf::from(p);
        if pb.is_dir() {
            return Some(pb);
        }
    }
    if let Some(repo) = resolve_source_repo() {
        let pb = repo.join("prompts");
        if pb.is_dir() {
            return Some(pb);
        }
    }
    None
}

fn load_prompt(rel: &str, embedded: &str) -> String {
    if let Some(dir) = prompts_override_dir() {
        let path = dir.join(rel);
        if let Ok(s) = fs::read_to_string(&path) {
            return s;
        }
    }
    embedded.to_string()
}

fn trim_trailing_newline(s: String) -> String {
    let trimmed = s.trim_end_matches('\n');
    if trimmed.len() == s.len() {
        s
    } else {
        trimmed.to_string()
    }
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
/// Canvas / drawing board, Inbox, Lineage pipeline) MUST survive every pass.
pub fn iteration_guidance(iteration: u32) -> String {
    let n = iteration.max(1);
    let port = iteration_port(n);
    let (phase, rel, embedded) = match n {
        1..=5 => ("Bootstrap phase", "phases/bootstrap.md", EMBEDDED_PHASE_BOOTSTRAP),
        6..=9 => ("Shaping phase", "phases/shaping.md", EMBEDDED_PHASE_SHAPING),
        10..=12 => (
            "Consolidation phase",
            "phases/consolidation.md",
            EMBEDDED_PHASE_CONSOLIDATION,
        ),
        _ => ("Maturation phase", "phases/maturation.md", EMBEDDED_PHASE_MATURATION),
    };
    let latitude = trim_trailing_newline(load_prompt(rel, embedded));

    let template = load_prompt("iteration_guidance.md", EMBEDDED_ITERATION_GUIDANCE);
    template
        .replace("{n}", &n.to_string())
        .replace("{phase}", phase)
        .replace("{latitude}", &latitude)
        .replace("{port}", &port.to_string())
}

#[cfg(any())]
const _DEAD_TEMPLATE: &str = r#"# Iteration {n} — {phase}

This Evolvo instance is a self-evolving meta-app. Each approved lineage job is one iteration in the life of the app the user is building on top of the Evolvo shell.

**Latitude for this iteration:** {latitude}

## Invariants you MUST preserve on every iteration, no matter what

Whatever the app becomes, the shell must keep these four surfaces reachable and functional:

1. **Feedback Overlay** — reachable from every screen, every mode. The user must always be able to open the feedback panel and submit new feedback about the page they are on.
2. **Canvas overlay on every page** — the Canvas is NOT a standalone tab or dedicated route. It is an overlay the user can open on top of *any* page of the app to draw / annotate / sketch feedback about *that specific page*. Every route must support opening the Canvas on top of it; the feedback submission records which route the drawing was made on. A design that only lets the user draw on a single "Canvas tab" is wrong — the whole point is per-page visual feedback.
3. **Inbox** — the list/overview of submitted feedback must remain visible and navigable, and each entry must preserve the page/route it was submitted from.
4. **Lineage pipeline** — the feedback → lineage-job state machine (and the Advance / Retry / Reject / Run affordances) must keep working end-to-end so the *next* iteration can happen.

If your change would break any of these four surfaces in the resulting app, it is wrong — redesign the change to preserve them. These invariants are load-bearing; they are what makes iteration N+1 possible.

## ONE button opens BOTH the Canvas overlay AND the Feedback panel — always

This is a hard rule, not a suggestion. The host iteration zero ships a **single Feedback FAB** (`FeedbackFab` in `app/ui/src/shell.rs`) bound to a **single `panel_open: RwSignal<bool>`** signal owned by the invariant shell. Clicking it toggles the feedback surface open; while open, both the drawing surface (Canvas + Toolbar) and the Feedback submission panel are visible and usable together. There is never one button for "draw" and a second button for "send feedback" — they are the same action from the user's point of view.

## Where the NewApp goes — `app.rs` is yours, `shell.rs` is not

The Leptos UI is split into two layers:

- **`app/ui/src/shell.rs`** is the permanent Evolvo chrome: the app bar with the Lineage navigation + "Star Us" link, the Lineage review page, the single Feedback FAB, and the Canvas overlay + feedback panel composition. The shell is what guarantees the four invariants above — the FAB and overlay wrap whatever content renders inside. **`shell.rs` is invariant.** Do not delete, duplicate, or re-implement any of its pieces inside the NewApp. If the chrome genuinely needs to change, edit `shell.rs` directly and keep all four surfaces working.
- **`app/ui/src/app.rs`** is the **NewApp content area**. When the user asks for a new app, this is where you build it: replace `HomePage` with the new app's root component (router, layout, pages, state) and add further modules alongside it. Keep `App` mounting `<Shell>` with the new content as its children. Because the shell wraps the content, every page/route of the NewApp is automatically annotatable — the Canvas overlay mounts on top of whatever `app.rs` renders when the user clicks the FAB.

If your NewApp needs to react to the Canvas being open (for example, to hide copy that shouldn't appear in the submission screenshot), read `PanelOpen` from context — the shell provides it via `provide_context`. Do not re-implement the FAB or the `panel_open` signal inside `app.rs`.

If you rewrite the UI stack off Leptos entirely, reproduce the same split in the replacement: a permanent shell module that owns the four invariant surfaces, and a NewApp content module mounted inside it.

Rules you MUST follow when the iteration app keeps a Feedback affordance (i.e. always):

- **Exactly one trigger.** One FAB, one toolbar button, one keyboard shortcut, one menu item — pick *one* affordance per surface. Do NOT ship "the old Canvas button that no longer works" alongside "a new FAB for the feedback container". If you rewrite, DELETE the previous trigger in the same change. Two buttons where the user can't tell which one is live = broken.
- **One signal drives both.** Bind the Canvas overlay's visibility and the Feedback panel's visibility to the **same** `RwSignal<bool>` (equivalent in whatever stack you're on). When it flips true, both surfaces come up; when it flips false, both go away. No "half-open" state where Canvas is up but Feedback isn't, or vice-versa.
- **Discoverable on every page.** The trigger is visible on every route — floating, pinned, or in a persistent chrome region — never hidden behind a tab switch or a hover-only menu. Icon-only triggers MUST have `aria-label` (and a `title`) so the user knows what they do.
- **Clearly labelled.** The user must be able to tell *at a glance* what that single button does. Iteration zero uses "Send feedback" as the `title` and `aria-label`, a pencil/close icon, and a count badge when there are pending annotations. Keep that intent: the button's label must name the feedback/annotation action explicitly, not just show a glyph.
- **Delete dead triggers.** If you restyle or move the affordance, remove the prior one in the same commit. A deprecated button that "still renders but does nothing" is a regression — the user will click it first, get nothing, and file feedback about *that*.

Concretely: if the user sees two buttons and isn't sure which one opens feedback, you have already failed this invariant. Redesign until there is exactly one.

## Context hygiene — update docs and agents alongside the code

Because future iterations rely on the repo's own documentation for context, any non-trivial change to the app MUST also update:

- `CLAUDE.md` — reflect the new architecture, stack, commands, domain model. Remove stale sections rather than layering on top.
- `.claude/rules/` — update conventions that no longer match the code (or add new ones). Delete rules for layers that no longer exist.
- `.claude/agents/*` — if an agent's description, responsibilities, or tools no longer match the current codebase, update its frontmatter and body. If a whole agent is obsolete, delete it; if the app now needs a new specialist, add one.
- `.claude/skills/*` (if present) — same treatment: keep them accurate or remove them.

The next iteration's agent will read these files first. Leaving them stale is the single biggest way to sabotage iteration N+1.

## Per-iteration dev-server port

This iteration's dev server MUST listen on **port {port}** (base `1530` + iteration `{n}`). The runner has already rewritten `app/src-tauri/tauri.conf.json`, `app/ui/Trunk.toml`, and `app/ui/scripts/trunk-dev.sh` in this worktree to use port `{port}` so concurrent iteration runs don't collide. When the reviewer clicks **Run**, the runner also sets `EVOLVO_ITERATION_PORT={port}` in the child environment.

If you rewrote the stack so the default files no longer exist, you MUST honour `EVOLVO_ITERATION_PORT` in `scripts/run-iteration.sh` (or whatever startup script you ship) and bind the dev/server on that port. Never hardcode `1530` — it belongs to the host Evolvo.

## Keep the iteration runnable — `scripts/run-iteration.sh`

The reviewer UI has a **Run** button that launches the app built in this iteration's worktree. It invokes `scripts/run-iteration.sh` at the worktree root if present, otherwise falls back to `cargo tauri dev` in `app/src-tauri`.

If you rewrite the stack (e.g. move off Tauri/Leptos) you MUST create or update `scripts/run-iteration.sh` so the Run button still works. The script should:

- Start the current app in the foreground (the runner streams its stdout/stderr into a log file).
- Bind the dev/server to `$EVOLVO_ITERATION_PORT` (falling back to `{port}` for this iteration if the env var isn't set).
- Respect `EVOLVO_WORKSPACE_ROOT` if the app stores any state — the runner sets that env var to a per-iteration workspace directory so runs stay isolated from the host Evolvo.
- Exit non-zero on startup failure so the reviewer sees a useful error in the lineage notes.

If you kept the default stack, you can skip the script and rely on the `cargo tauri dev` fallback.

## Verify-before-done — you MUST run the app before calling the task complete

Type-checking is not verification. Before you commit and return, you MUST actually start the app and confirm it boots. The reviewer expects a running binary, not a green `cargo check`.

Concrete steps for the default stack (adapt for whatever stack this iteration ships):

1. Run `cargo check -p evolvo_desktop` and `cargo check -p evolvo_ui --target wasm32-unknown-unknown`. Both must pass.
2. Run `cargo test -p evolvo_desktop` and fix any regression you introduced. Add tests for new host-side logic.
3. Start the app in the background: `EVOLVO_ITERATION_PORT={port} cargo tauri dev` (or `bash scripts/run-iteration.sh`). Wait for the dev server to print its ready line (Trunk prints `server listening at http://127.0.0.1:{port}`). If the build fails or the server doesn't come up, fix the cause — do NOT claim success.
4. Exercise the change: navigate to the affected route, trigger the feedback / canvas / lineage path that the feedback is about, and confirm the user-visible behaviour matches what was asked for. Try to break it — empty inputs, fast clicks, edge cases adjacent to what the feedback described. If any of the four invariants (Feedback Overlay, per-page Canvas overlay, Inbox, Lineage pipeline) regressed, that's a blocker: fix it before finishing.
5. Only after the app actually ran and the change actually worked, commit and return.

If you genuinely cannot run the app in this environment (no display, missing system deps), say so plainly in your final summary — don't fake it. "I couldn't run the app because X" is acceptable; "looks good, tests pass" when you never started the binary is not.

## After implementation — commit, then start the new version

When the change is verified:

1. Stage and commit every file you touched (including updated `CLAUDE.md` / rules / agents). Use a conventional-commit subject like `feat(ui): <short>` or `fix(lineage): <short>`. One focused commit is fine; multiple small commits are better when the work naturally splits.
2. Leave the iteration's app running so the reviewer lands on a live build. If you shut it down earlier to rebuild, start it again before returning: `EVOLVO_ITERATION_PORT={port} cargo tauri dev` (or the equivalent for your stack). The reviewer's Run button will also launch it, but starting it here saves them a click and confirms startup worked.
3. In your final summary mention the port this iteration is serving on ({port}) and how you verified the change.
"#;

pub fn build_implementation_prompt(
    feedback: &FeedbackRecord,
    job: &LineageJobRecord,
    attachments: &[StagedAttachment],
    log_file: &Path,
) -> String {
    // Keep the historical signature green; callers that don't care about the
    // agent get ClaudeCode phrasing, which is still a valid instruction for
    // any CLI (they all recognise "read the project guide" regardless).
    build_implementation_prompt_for(
        feedback,
        job,
        attachments,
        log_file,
        AgentKind::ClaudeCode,
    )
}

/// Agent-aware prompt builder. Substitutes `{agent_label}` and
/// `{agent_context_files}` so the same template nudges Codex / Gemini /
/// Forge to read `AGENTS.md` / `GEMINI.md` while keeping Claude on
/// `CLAUDE.md`.
pub fn build_implementation_prompt_for(
    feedback: &FeedbackRecord,
    job: &LineageJobRecord,
    attachments: &[StagedAttachment],
    log_file: &Path,
    agent: AgentKind,
) -> String {
    let iteration = if job.iteration == 0 { 1 } else { job.iteration };
    let is_new_app = matches!(feedback.feedback_type, crate::types::FeedbackType::NewApp);
    let guidance = iteration_guidance(iteration);
    // A `NewApp` feedback is an explicit "start over" signal from the user —
    // it overrides the iteration-number-based latitude and forces the agent
    // back into bootstrap mode no matter how many iterations have happened.
    let (step_rel, step_embed) = if is_new_app {
        ("work_steps/new_app.md", EMBEDDED_STEP_NEW_APP)
    } else if iteration <= 5 {
        ("work_steps/bootstrap.md", EMBEDDED_STEP_BOOTSTRAP)
    } else if iteration <= 10 {
        ("work_steps/shaping.md", EMBEDDED_STEP_SHAPING)
    } else {
        ("work_steps/maturation.md", EMBEDDED_STEP_MATURATION)
    };
    let work_step_4 = trim_trailing_newline(load_prompt(step_rel, step_embed));
    let new_app_banner = if is_new_app {
        load_prompt("new_app_banner.md", EMBEDDED_NEW_APP_BANNER)
    } else {
        String::new()
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
        let mut s = String::from(
            "\n\n## Attachments (read these with the Read tool before planning/implementing)\n",
        );
        for a in attachments {
            s.push_str(&format!("- **{}** — `{}`\n", a.role, a.path.display()));
        }
        s
    };

    let template = load_prompt("implementation.md", EMBEDDED_IMPLEMENTATION);
    let backend = backend_for(agent);
    let context_files = backend.context_files().to_vec().join(", ");
    template
        .replace("{guidance}", &guidance)
        .replace("{new_app_banner}", &new_app_banner)
        .replace("{job_id}", &job.id)
        .replace("{branch}", &branch_name(&job.id))
        .replace("{iteration}", &iteration.to_string())
        .replace("{title}", &job.title)
        .replace("{feedback_type}", &feedback_type)
        .replace("{route}", &route)
        .replace("{feedback_text}", &feedback.feedback_text)
        .replace("{voice_line}", &voice_line)
        .replace("{attachments_section}", &attachments_section)
        .replace("{work_step_4}", &work_step_4)
        .replace("{log_file}", &log_file.display().to_string())
        .replace("{agent_label}", agent.label())
        .replace("{agent_context_files}", &context_files)
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
/// files. Does NOT launch the agent — that happens in
/// [`launch_agent_session`], so the caller has the chance to persist
/// artifact paths onto the job record first. The `agent` argument picks
/// the log filename, the prompt's `{agent_context_files}` substitution,
/// and which worktree-root context-file aliases get materialised.
pub fn prepare_run(
    store: &Store,
    job: &LineageJobRecord,
    feedback: &FeedbackRecord,
    agent: AgentKind,
) -> Result<PreparedRun, StoreError> {
    // Lazily allocate an iteration number for this job. We work on a local
    // copy so the caller's `&LineageJobRecord` signature stays intact; the
    // allocated iteration is persisted back onto the stored record so the
    // UI (and any retry) sees a stable value.
    let mut job = job.clone();
    if job.iteration == 0 {
        let n = store.allocate_iteration()?;
        job.iteration = n;
        store.save_lineage_job(&job)?;
    }
    let job = &job;
    let source = resolve_source_repo().ok_or_else(|| {
        StoreError::Other(
            "could not locate Evolvo source repo — set EVOLVO_SOURCE_REPO or run from within the repo"
                .to_string(),
        )
    })?;

    let root = store.layout().root().to_path_buf();
    let job_dir = job_workspace_dir(&root, &job.id);
    fs::create_dir_all(&job_dir)?;

    let worktree = worktree_path(&root, &job.id);
    let inputs_dir = inputs_path(&root, &job.id);
    let log_file = agent_log_path(&root, &job.id, agent);
    let prompt_file = prompt_path(&root, &job.id);
    let metadata_file = metadata_path(&root, &job.id);
    let branch = branch_name(&job.id);

    create_worktree(&source, &worktree, &branch)?;
    let attachments = stage_attachments(store, feedback, &inputs_dir)?;

    // Drop agent-appropriate context aliases (e.g. `AGENTS.md` → CLAUDE.md
    // for Codex/Forge, `GEMINI.md` for Gemini) so each CLI picks up the
    // same project guide under the name it expects. Best-effort; failures
    // are swallowed because the worktree is already usable without them.
    let backend = backend_for(agent);
    let aliases = materialise_context_files(&worktree, backend.as_ref());

    // Each iteration gets its own dev-server port so users can run multiple
    // iterations side-by-side without collision. Rewrite the worktree's port
    // config before the agent starts so `cargo tauri dev` "just works".
    let port = iteration_port(job.iteration);
    let _ = rewrite_iteration_port(&worktree, port);

    let prompt = build_implementation_prompt_for(feedback, job, &attachments, &log_file, agent);
    fs::write(&prompt_file, &prompt)?;

    let metadata = serde_json::json!({
        "job_id": job.id,
        "feedback_id": feedback.id,
        "iteration": job.iteration,
        "iteration_port": port,
        "branch": branch,
        "worktree": worktree.display().to_string(),
        "source_repo": source.display().to_string(),
        "log_file": log_file.display().to_string(),
        "prompt_file": prompt_file.display().to_string(),
        "agent": agent,
        "agent_binary": backend.binary(),
        "agent_context_aliases": aliases.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
        "attachments": attachments.iter().map(|a| serde_json::json!({
            "role": a.role,
            "path": a.path.display().to_string(),
        })).collect::<Vec<_>>(),
        "permission_mode": "dangerously-skip-permissions",
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

/// Spawn the selected agent's CLI in `worktree`, stream its output to
/// `log_file`, and transition the job when it finishes. Returns
/// immediately; all I/O happens on a dedicated OS thread. The `agent`
/// argument picks the binary, the flag set, and the env scrub list — see
/// [`crate::agent`].
pub fn launch_agent_session(
    store: Store,
    job_id: String,
    prepared: PreparedRun,
    agent: AgentKind,
) {
    std::thread::spawn(move || {
        let engine = LineageEngine::new(&store);
        let backend = backend_for(agent);

        let log_handle = match fs::File::create(&prepared.log_file) {
            Ok(f) => f,
            Err(e) => {
                let _ = engine.append_note(
                    &job_id,
                    &format!("failed to open log {}: {e}", prepared.log_file.display()),
                );
                let _ = engine.force_status(&job_id, LineageJobStatus::Failed);
                return;
            }
        };
        let log_for_err = match log_handle.try_clone() {
            Ok(f) => f,
            Err(e) => {
                let _ = engine.append_note(&job_id, &format!("failed to clone log handle: {e}"));
                let _ = engine.force_status(&job_id, LineageJobStatus::Failed);
                return;
            }
        };

        let _ = engine.append_note(
            &job_id,
            &format!(
                "{agent_label} starting ({binary}) in worktree {} — transcript → {}",
                prepared.worktree.display(),
                prepared.log_file.display(),
                agent_label = agent.label(),
                binary = backend.binary(),
            ),
        );

        let status = backend
            .build_command(&prepared.prompt, &prepared.worktree)
            .stdout(Stdio::from(log_handle))
            .stderr(Stdio::from(log_for_err))
            .status();

        match status {
            Ok(s) if s.success() => {
                let _ = engine.append_note(
                    &job_id,
                    &format!("{} completed successfully", agent.label()),
                );
                let _ = engine.force_status(&job_id, LineageJobStatus::BuildReady);
            }
            Ok(s) => {
                let _ = engine.append_note(
                    &job_id,
                    &format!(
                        "{} exited with status {s} — see log {}",
                        agent.label(),
                        prepared.log_file.display()
                    ),
                );
                let _ = engine.force_status(&job_id, LineageJobStatus::Failed);
            }
            Err(e) => {
                let _ = engine.append_note(
                    &job_id,
                    &format!(
                        "failed to launch {} ({e}) — ensure the `{}` CLI is installed and in PATH",
                        agent.label(),
                        backend.binary(),
                    ),
                );
                let _ = engine.force_status(&job_id, LineageJobStatus::Failed);
            }
        }
    });
}

/// Backward-compatible shim so old callers (and any external code the
/// workspace still references) keep compiling. Forwards to
/// [`launch_agent_session`] with the Claude backend.
#[deprecated(note = "use launch_agent_session(..., AgentKind) so the runner can honour the selected agent")]
pub fn launch_claude(store: Store, job_id: String, prepared: PreparedRun) {
    launch_agent_session(store, job_id, prepared, AgentKind::ClaudeCode);
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
/// the default Evolvo shell location (`app/src-tauri`).
pub struct ResolvedRunCommand {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub source: &'static str,
}

/// Build a PATH suitable for spawning developer toolchains (`cargo`, `trunk`,
/// `tauri-cli`, `node`, `pnpm`…) from a GUI-launched host. macOS apps started
/// from Finder/Dock inherit a minimal `/usr/bin:/bin:/usr/sbin:/sbin`, which
/// is why `cargo tauri dev` silently fails with "No such file or directory"
/// when the Run button is clicked from the packaged app. We prepend the
/// common install locations so the child can actually find its tools.
pub fn enriched_path() -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        parts.push(format!("{home}/.cargo/bin"));
        parts.push(format!("{home}/.local/bin"));
        parts.push(format!("{home}/.bun/bin"));
        parts.push(format!("{home}/.volta/bin"));
        parts.push(format!("{home}/.nvm/versions/node/current/bin"));
    }
    parts.push("/opt/homebrew/bin".into());
    parts.push("/opt/homebrew/sbin".into());
    parts.push("/usr/local/bin".into());
    parts.push("/usr/local/sbin".into());
    if let Ok(existing) = std::env::var("PATH") {
        if !existing.is_empty() {
            parts.push(existing);
        }
    }
    parts.push("/usr/bin:/bin:/usr/sbin:/sbin".into());
    parts.join(":")
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

/// Spawn the iteration's app from its lineage worktree and stream output to
/// `iteration-run.log` under the job workspace. Fire-and-forget: the call
/// returns as soon as the child is handed off to a dedicated thread; the
/// status the user sees in the UI reflects the lineage state machine, not
/// the run process itself. The child gets its own `EVOLVO_WORKSPACE_ROOT`
/// pointed at the per-job `run_workspace/` dir so it can't clobber the host
/// Evolvo's feedback / lineage data.
pub fn launch_iteration_run(store: Store, job_id: String) {
    std::thread::spawn(move || {
        let engine = LineageEngine::new(&store);

        let Some(job) = store.load_lineage_job(&job_id).ok().flatten() else {
            let _ = engine.append_note(&job_id, "run requested but lineage job record is missing");
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
                let _ =
                    engine.append_note(&job_id, &format!("failed to clone run log handle: {e}"));
                return;
            }
        };

        let cmd = resolve_run_command(&worktree);
        let port = iteration_port(job.iteration);
        let path_env = enriched_path();

        // Write a preflight banner to the log file BEFORE spawning so the
        // user has context even if the child process never manages to boot
        // (missing toolchain, missing cwd, bad permissions, etc.). `mut`
        // because we reopen it as append after spawn.
        {
            let mut banner = match log_handle.try_clone() {
                Ok(f) => f,
                Err(_) => log_handle
                    .try_clone()
                    .unwrap_or_else(|_| fs::File::create(&log_path).unwrap()),
            };
            let _ = writeln!(
                banner,
                "== iteration run ==\njob_id: {}\niteration: {}\nport: {}\ncommand: {} {}\ncwd: {}\nsource: {}\nPATH: {}\n--",
                job_id,
                job.iteration,
                port,
                cmd.program,
                cmd.args.join(" "),
                cmd.cwd.display(),
                cmd.source,
                path_env,
            );
        }

        // Fast-fail checks surface a clear error to the user instead of a
        // silent "thread exited" — both into the notes (so the card shows
        // it after refresh) and into the log file (for a permanent record).
        if !cmd.cwd.exists() {
            let msg = format!(
                "run failed: cwd {} does not exist — did this iteration rewrite the stack without adding scripts/run-iteration.sh?",
                cmd.cwd.display()
            );
            let _ = writeln!(&log_handle, "{msg}");
            let _ = engine.append_note(&job_id, &msg);
            return;
        }

        let _ = engine.append_note(
            &job_id,
            &format!(
                "launching iteration run via {} ({} {}) in {} on port {} — log {}",
                cmd.source,
                cmd.program,
                cmd.args.join(" "),
                cmd.cwd.display(),
                port,
                log_path.display(),
            ),
        );

        let status = Command::new(&cmd.program)
            .args(&cmd.args)
            .current_dir(&cmd.cwd)
            .env("PATH", &path_env)
            .env("EVOLVO_WORKSPACE_ROOT", &run_workspace)
            .env("EVOLVO_ITERATION_PORT", port.to_string())
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
                let hint = if e.kind() == std::io::ErrorKind::NotFound {
                    format!(
                        "`{}` was not found on PATH. On macOS, apps launched from Finder/Dock inherit a minimal PATH; install the toolchain or launch Evolvo from a shell where `{}` works. PATH used: {}",
                        cmd.program, cmd.program, path_env
                    )
                } else {
                    format!("spawn error: {e}")
                };
                let _ = engine
                    .append_note(&job_id, &format!("failed to launch iteration run — {hint}"));
            }
        }
    });
}

// --- Multi-stage NewApp pipeline dispatcher ---------------------------------

/// Stage prompts live alongside the classic ones under `prompts/stages/`.
/// Resolved with the same override chain: `$EVOLVO_PROMPTS_DIR`, source repo,
/// then the compiled-in fallback.
fn stage_prompt_template(kind: crate::types::StageKind) -> String {
    use crate::types::StageKind as K;
    let (rel, embedded) = match kind {
        K::BackendPlan => ("stages/backend_plan.md", EMBEDDED_STAGE_BACKEND_PLAN),
        K::BackendImpl => ("stages/backend_impl.md", EMBEDDED_STAGE_BACKEND_IMPL),
        K::FrontendPlan => ("stages/frontend_plan.md", EMBEDDED_STAGE_FRONTEND_PLAN),
        K::FrontendImpl => ("stages/frontend_impl.md", EMBEDDED_STAGE_FRONTEND_IMPL),
        K::E2EPlan => ("stages/e2e_plan.md", EMBEDDED_STAGE_E2E_PLAN),
        K::E2EImpl => ("stages/e2e_impl.md", EMBEDDED_STAGE_E2E_IMPL),
        K::FinalReview => ("stages/final_review.md", EMBEDDED_STAGE_FINAL_REVIEW),
    };
    load_prompt(rel, embedded)
}

/// Render the concrete per-stage prompt by substituting `{{VAR}}` placeholders
/// with plan + job-derived values.
fn render_stage_prompt(
    template: &str,
    plan: &crate::plan::IterationPlan,
    worktree: &Path,
    plan_path: &Path,
    job_id: &str,
) -> String {
    let canvas_png = plan
        .canvas
        .png_path
        .clone()
        .unwrap_or_else(|| "<none>".to_string());
    let annotations = plan
        .canvas
        .annotations_path
        .clone()
        .unwrap_or_else(|| "<none>".to_string());
    let voice = plan
        .voice_transcript
        .clone()
        .unwrap_or_else(|| "<none>".to_string());
    let region_index = if plan.canvas.regions.is_empty() {
        "(no annotations — text-only feedback)".to_string()
    } else {
        plan.canvas
            .regions
            .iter()
            .map(|r| {
                format!(
                    "{}: bbox=[{:.0},{:.0},{:.0},{:.0}] strokes={} color={} labels={:?}",
                    r.id,
                    r.bbox[0],
                    r.bbox[1],
                    r.bbox[2],
                    r.bbox[3],
                    r.stroke_count,
                    r.dominant_color.as_deref().unwrap_or("-"),
                    r.labels
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    template
        .replace("{{APP_NAME}}", &plan.app.name)
        .replace("{{JOB_ID}}", job_id)
        .replace("{{ITERATION}}", &plan.app.iteration.to_string())
        .replace("{{ROUTE}}", &plan.canvas.route)
        .replace("{{PLAN_PATH}}", &plan_path.display().to_string())
        .replace("{{CANVAS_PNG}}", &canvas_png)
        .replace("{{ANNOTATIONS_PATH}}", &annotations)
        .replace("{{USER_TEXT}}", &plan.user_text)
        .replace("{{VOICE_TRANSCRIPT}}", &voice)
        .replace("{{REGION_INDEX}}", &region_index)
        .replace("{{WORKTREE}}", &worktree.display().to_string())
}

/// Concrete `StageDispatcher` that spawns the selected agent CLI per
/// stage and streams its output to `<job_dir>/logs/<slug>.log`. Blocks the
/// calling thread until the agent exits.
pub struct AgentStageDispatcher {
    pub agent: AgentKind,
}

impl AgentStageDispatcher {
    pub fn new(agent: AgentKind) -> Self {
        Self { agent }
    }
}

impl crate::stages::StageDispatcher for AgentStageDispatcher {
    fn dispatch(&self, ctx: crate::stages::StageDispatch<'_>) -> Result<PathBuf, String> {
        let plan = crate::plan::load_plan(ctx.job_dir)
            .map_err(|e| format!("load plan: {e}"))?
            .ok_or_else(|| "plan.json missing before stage dispatch".to_string())?;

        let logs_dir = ctx.job_dir.join("logs");
        fs::create_dir_all(&logs_dir).map_err(|e| format!("create logs dir: {e}"))?;
        let log_path = logs_dir.join(format!("{}.log", ctx.stage.slug()));
        let log_handle = fs::File::create(&log_path)
            .map_err(|e| format!("create log {}: {e}", log_path.display()))?;
        let log_err = log_handle
            .try_clone()
            .map_err(|e| format!("clone log handle: {e}"))?;

        let template = stage_prompt_template(ctx.stage);
        let prompt = render_stage_prompt(&template, &plan, ctx.worktree, &ctx.plan_path, ctx.job_id);

        let backend = backend_for(self.agent);
        let status = backend
            .build_command(&prompt, ctx.worktree)
            .stdout(Stdio::from(log_handle))
            .stderr(Stdio::from(log_err))
            .status()
            .map_err(|e| format!("spawn {} for {}: {e}", backend.binary(), ctx.stage.slug()))?;

        if !status.success() {
            return Err(format!(
                "{} exited {status} for stage {} — see {}",
                self.agent.label(),
                ctx.stage.slug(),
                log_path.display()
            ));
        }
        Ok(log_path)
    }
}

/// Back-compat alias retained for any external code that still refers to
/// the old name. New code should construct [`AgentStageDispatcher::new`].
#[deprecated(note = "use AgentStageDispatcher::new(AgentKind)")]
pub type ClaudeStageDispatcher = AgentStageDispatcher;

/// Run the multi-stage NewApp pipeline for a prepared job. Seeds the plan,
/// executes every stage, and flips the job to `BuildReady` on success or
/// `Failed` on the first red stage. Spawned on its own OS thread so the
/// Tauri command returns immediately.
pub fn launch_pipeline(
    store: Store,
    job_id: String,
    feedback: FeedbackRecord,
    prepared: PreparedRun,
    agent: AgentKind,
) {
    std::thread::spawn(move || {
        let engine = LineageEngine::new(&store);
        let root = store.layout().root().to_path_buf();
        let job_dir = job_workspace_dir(&root, &job_id);

        let job = match store.load_lineage_job(&job_id) {
            Ok(Some(j)) => j,
            _ => {
                let _ = engine.append_note(&job_id, "pipeline: job record vanished");
                return;
            }
        };

        if let Err(e) =
            crate::stages::seed_plan_from_feedback(&store, &feedback, &job_id, &job_dir, job.iteration)
        {
            let _ = engine.append_note(&job_id, &format!("pipeline: seed plan failed — {e}"));
            let _ = engine.force_status(&job_id, LineageJobStatus::Failed);
            return;
        }

        if let Err(e) = crate::stages::snapshot_styles_baseline(&prepared.worktree, &job_dir) {
            let _ = engine.append_note(
                &job_id,
                &format!("pipeline: styles.css baseline snapshot failed — {e} (continuing; delta gate will be skipped)"),
            );
        }

        let _ = engine.append_note(
            &job_id,
            &format!(
                "multi-stage NewApp pipeline starting in worktree {} using {} — stages: {}",
                prepared.worktree.display(),
                agent.label(),
                crate::types::StageKind::pipeline()
                    .iter()
                    .map(|s| s.slug())
                    .collect::<Vec<_>>()
                    .join(" → ")
            ),
        );

        let dispatcher = AgentStageDispatcher::new(agent);
        match crate::stages::run_pipeline(
            &store,
            &engine,
            &dispatcher,
            &prepared.worktree,
            &job_dir,
            &job_id,
        ) {
            Ok(stages) => {
                let all_green = stages.iter().all(|s| {
                    matches!(
                        s.status,
                        crate::types::StageStatus::Green | crate::types::StageStatus::Skipped
                    )
                });
                if all_green {
                    let _ = engine
                        .append_note(&job_id, "pipeline: all stages green — marking build ready");
                    let _ = engine.force_status(&job_id, LineageJobStatus::BuildReady);
                } else {
                    let first_red = stages
                        .iter()
                        .find(|s| matches!(s.status, crate::types::StageStatus::Failed))
                        .map(|s| s.kind.slug())
                        .unwrap_or("<unknown>");
                    let _ = engine.append_note(
                        &job_id,
                        &format!("pipeline: stopped at {first_red} — marking failed"),
                    );
                    let _ = engine.force_status(&job_id, LineageJobStatus::Failed);
                }
            }
            Err(e) => {
                let _ = engine.append_note(&job_id, &format!("pipeline: dispatcher error — {e}"));
                let _ = engine.force_status(&job_id, LineageJobStatus::Failed);
            }
        }
    });
}

/// True iff the feedback is a NewApp request carrying a canvas screenshot or
/// annotations. Used by the command layer to route to `launch_pipeline`
/// instead of the single-session `launch_claude`.
pub fn is_multi_stage_candidate(feedback: &FeedbackRecord) -> bool {
    matches!(feedback.feedback_type, crate::types::FeedbackType::NewApp)
        && (feedback.screenshot_filename.is_some() || !feedback.annotations.is_empty())
}

/// Resume an interrupted multi-stage pipeline. Unlike `launch_pipeline`,
/// this does NOT create a new worktree or rewrite the port — it reuses the
/// worktree already on disk at `job.worktree_path` and re-enters the
/// pipeline. `run_stage` consults `plan.stage` and skips dispatch for
/// stages whose output is already persisted, so the net effect is: retry
/// the first stage that did not reach `Green` last time.
pub fn resume_pipeline(store: Store, job_id: String) -> Result<(), String> {
    let job = store
        .load_lineage_job(&job_id)
        .map_err(|e| format!("load job: {e}"))?
        .ok_or_else(|| format!("lineage job not found: {job_id}"))?;

    let feedback = store
        .load_feedback(&job.feedback_id)
        .map_err(|e| format!("load feedback: {e}"))?
        .ok_or_else(|| format!("feedback {} not found for job {job_id}", job.feedback_id))?;

    let worktree = job
        .worktree_path
        .as_ref()
        .map(PathBuf::from)
        .ok_or_else(|| "job has no worktree_path — cannot resume; retry the job instead".to_string())?;

    if !worktree.exists() {
        return Err(format!(
            "worktree {} no longer exists — retry the job instead of resuming",
            worktree.display()
        ));
    }

    let root = store.layout().root().to_path_buf();
    let job_dir = job_workspace_dir(&root, &job_id);
    if !job_dir.join(crate::plan::PLAN_FILENAME).exists() {
        return Err(format!(
            "no plan.json under {} — nothing to resume",
            job_dir.display()
        ));
    }

    std::thread::spawn(move || {
        let engine = LineageEngine::new(&store);
        // The caller (`commands::resume_lineage_job`) has already flipped
        // status to `Implementing` synchronously before spawning this
        // thread — don't re-flip it here, that would race with whatever
        // else updates the record (e.g. `update_stage` from the first
        // stage that runs below).
        let _ = engine.append_note(
            &job_id,
            &format!(
                "resume: re-entering pipeline ({agent_label}) in worktree {} — stages already green will be skipped",
                worktree.display(),
                agent_label = job.agent.label(),
            ),
        );

        // Seed is idempotent: returns the existing plan when one is on disk.
        if let Err(e) =
            crate::stages::seed_plan_from_feedback(&store, &feedback, &job_id, &job_dir, job.iteration)
        {
            let _ = engine.append_note(&job_id, &format!("resume: seed plan failed — {e}"));
            let _ = engine.force_status(&job_id, LineageJobStatus::Failed);
            return;
        }

        // Idempotent: keeps the existing baseline when one is already on disk
        // so resume doesn't re-capture a post-mutation styles.css as the
        // "baseline".
        if let Err(e) = crate::stages::snapshot_styles_baseline(&worktree, &job_dir) {
            let _ = engine.append_note(
                &job_id,
                &format!("resume: styles.css baseline snapshot failed — {e} (continuing)"),
            );
        }

        let dispatcher = AgentStageDispatcher::new(job.agent);
        match crate::stages::run_pipeline(
            &store,
            &engine,
            &dispatcher,
            &worktree,
            &job_dir,
            &job_id,
        ) {
            Ok(stages) => {
                let all_green = stages.iter().all(|s| {
                    matches!(
                        s.status,
                        crate::types::StageStatus::Green | crate::types::StageStatus::Skipped
                    )
                });
                if all_green {
                    let _ =
                        engine.append_note(&job_id, "resume: all stages green — marking build ready");
                    let _ = engine.force_status(&job_id, LineageJobStatus::BuildReady);
                } else {
                    let first_red = stages
                        .iter()
                        .find(|s| matches!(s.status, crate::types::StageStatus::Failed))
                        .map(|s| s.kind.slug())
                        .unwrap_or("<unknown>");
                    let _ = engine.append_note(
                        &job_id,
                        &format!("resume: stopped at {first_red} — marking failed"),
                    );
                    let _ = engine.force_status(&job_id, LineageJobStatus::Failed);
                }
            }
            Err(e) => {
                let _ = engine.append_note(&job_id, &format!("resume: dispatcher error — {e}"));
                let _ = engine.force_status(&job_id, LineageJobStatus::Failed);
            }
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FeedbackStatus, FeedbackType};

    fn mk_feedback() -> FeedbackRecord {
        FeedbackRecord {
            id: "fb-42".into(),
            feedback_type: FeedbackType::Bug,
            status: FeedbackStatus::InLineage,
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
            lineage_job_id: Some("job-42".into()),
        }
    }

    fn mk_job() -> LineageJobRecord {
        LineageJobRecord {
            id: "job-42".into(),
            feedback_id: "fb-42".into(),
            title: "Save button is laggy".into(),
            summary: "…".into(),
            status: LineageJobStatus::Pending,
            notes: vec![],
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
            worktree_path: None,
            branch_name: None,
            log_path: None,
            source_repo: None,
            iteration: 0,
            stages: Vec::new(),
            agent: AgentKind::ClaudeCode,
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
        assert!(prompt.contains("lineage/job-42"));
        assert!(prompt.contains("Save button is laggy"));
        assert!(prompt.contains("The save button sometimes doesn't respond"));
        assert!(prompt.contains("the button feels laggy"));
        assert!(prompt.contains("canvas screenshot"));
        assert!(prompt.contains("/tmp/job-42/inputs/canvas.png"));
        assert!(prompt.contains("/tmp/job-42/inputs/annotations.json"));
        assert!(prompt.contains("/tmp/job-42/claude.log"));
        assert!(prompt.contains(".claude/agents/"));
        // Agent-neutral phrasing: the prompt tells the model it's running with
        // the CLI's permission-bypass flag. The literal flag text varies by
        // backend (see `crate::agent`), so we only assert the concept.
        assert!(prompt.contains("permission-bypass flag"));
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
            assert!(g.contains("Lineage"));
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
    fn prompt_treats_new_app_feedback_as_bootstrap_even_late_in_iterations() {
        let mut fb = mk_feedback();
        fb.feedback_type = FeedbackType::NewApp;
        let mut job = mk_job();
        job.iteration = 15; // Would normally be "minor, surgical" phase.

        let prompt = build_implementation_prompt(&fb, &job, &[], Path::new("/tmp/claude.log"));
        assert!(prompt.contains("NewApp"));
        assert!(prompt.contains("new app from scratch") || prompt.contains("NEW APP from scratch"));
        assert!(prompt.contains("overrides the iteration-phase latitude"));
        // The "minor, surgical" guidance must NOT be chosen for NewApp, even at iteration 15.
        assert!(!prompt.contains("Make the minimal, focused change"));
    }

    #[test]
    fn prompt_uses_job_iteration_when_set() {
        let mut job = mk_job();
        job.iteration = 10;
        let prompt =
            build_implementation_prompt(&mk_feedback(), &job, &[], Path::new("/tmp/claude.log"));
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
        fs::write(
            worktree.join("scripts").join("run-iteration.sh"),
            b"#!/bin/sh\n",
        )
        .unwrap();
        let chosen = resolve_run_command(&worktree);
        assert_eq!(chosen.program, "bash");
        assert!(chosen.args[0].ends_with("scripts/run-iteration.sh"));
        assert_eq!(chosen.cwd, worktree);
    }

    #[test]
    fn iteration_port_shifts_by_iteration_number() {
        assert_eq!(iteration_port(0), BASE_DEV_PORT);
        assert_eq!(iteration_port(1), BASE_DEV_PORT + 1);
        assert_eq!(iteration_port(7), BASE_DEV_PORT + 7);
        // Saturation guard: astronomical iterations stay in-range.
        assert!(iteration_port(u32::MAX) <= 65500);
    }

    #[test]
    fn rewrite_iteration_port_patches_known_files_and_tolerates_missing() {
        let temp = tempfile::tempdir().unwrap();
        let worktree = temp.path().to_path_buf();
        fs::create_dir_all(worktree.join("app/src-tauri")).unwrap();
        fs::create_dir_all(worktree.join("app/ui/scripts")).unwrap();

        fs::write(
            worktree.join("app/src-tauri/tauri.conf.json"),
            r#"{"build":{"devUrl":"http://localhost:1530"}}"#,
        )
        .unwrap();
        fs::write(worktree.join("app/ui/Trunk.toml"), "[serve]\nport = 1530\n").unwrap();
        // trunk-dev.sh intentionally omitted — rewrite should skip it cleanly.

        let port = iteration_port(3);
        let changed = rewrite_iteration_port(&worktree, port).unwrap();
        assert_eq!(changed.len(), 2);

        let conf = fs::read_to_string(worktree.join("app/src-tauri/tauri.conf.json")).unwrap();
        assert!(conf.contains(&port.to_string()));
        assert!(!conf.contains("1530"));

        let trunk = fs::read_to_string(worktree.join("app/ui/Trunk.toml")).unwrap();
        assert!(trunk.contains(&port.to_string()));
    }

    #[test]
    fn rewrite_iteration_port_is_noop_for_base_port() {
        let temp = tempfile::tempdir().unwrap();
        let worktree = temp.path().to_path_buf();
        fs::create_dir_all(worktree.join("app/src-tauri")).unwrap();
        fs::write(
            worktree.join("app/src-tauri/tauri.conf.json"),
            r#"{"devUrl":"http://localhost:1530"}"#,
        )
        .unwrap();
        let changed = rewrite_iteration_port(&worktree, BASE_DEV_PORT).unwrap();
        assert!(changed.is_empty());
    }

    #[test]
    fn iteration_guidance_enforces_single_trigger_for_canvas_and_feedback() {
        let g = iteration_guidance(1);
        assert!(g.contains("ONE button"));
        assert!(g.contains("Canvas overlay"));
        assert!(g.contains("Feedback panel"));
        assert!(g.contains("panel_open"));
        assert!(g.contains("Delete dead triggers") || g.contains("DELETE the previous trigger"));
    }

    #[test]
    fn iteration_guidance_mentions_port_and_verification() {
        let g = iteration_guidance(2);
        assert!(g.contains(&iteration_port(2).to_string()));
        assert!(g.contains("EVOLVO_ITERATION_PORT"));
        assert!(g.contains("Verify-before-done"));
        assert!(g.contains("commit"));
    }

    #[test]
    fn iteration_run_log_path_nests_under_job_workspace() {
        let root = PathBuf::from("/tmp/ws");
        let p = iteration_run_log_path(&root, "job-7");
        assert!(p.ends_with("lineage_workspaces/job-7/iteration-run.log"));
    }

    #[test]
    fn branch_name_uses_lineage_prefix() {
        assert_eq!(branch_name("job-123"), "lineage/job-123");
    }

    #[test]
    fn layout_paths_stay_under_workspace() {
        let root = PathBuf::from("/tmp/ws");
        let worktree = worktree_path(&root, "job-9");
        assert!(worktree.starts_with(&root));
        assert!(worktree.ends_with("lineage_workspaces/job-9/worktree"));

        let log = log_path(&root, "job-9");
        assert!(log.ends_with("lineage_workspaces/job-9/claude.log"));

        let inputs = inputs_path(&root, "job-9");
        assert!(inputs.ends_with("lineage_workspaces/job-9/inputs"));
    }

    #[test]
    fn resolve_source_repo_finds_evolvo_checkout() {
        let resolved = resolve_source_repo();
        assert!(
            resolved.is_some(),
            "should find the Evolvo source repo under the test harness"
        );
        let p = resolved.unwrap();
        assert!(p.join(".claude").join("agents").exists());
        assert!(p.join(".git").exists());
    }

    #[test]
    fn agent_log_path_uses_backend_filename() {
        let root = PathBuf::from("/tmp/ws");
        assert!(agent_log_path(&root, "job-1", AgentKind::ClaudeCode)
            .ends_with("lineage_workspaces/job-1/claude.log"));
        assert!(agent_log_path(&root, "job-1", AgentKind::CodexCli)
            .ends_with("lineage_workspaces/job-1/codex.log"));
        assert!(agent_log_path(&root, "job-1", AgentKind::GeminiCli)
            .ends_with("lineage_workspaces/job-1/gemini.log"));
        assert!(agent_log_path(&root, "job-1", AgentKind::Forge)
            .ends_with("lineage_workspaces/job-1/forge.log"));
    }

    #[test]
    fn prompt_for_codex_substitutes_agents_md_context() {
        let prompt = build_implementation_prompt_for(
            &mk_feedback(),
            &mk_job(),
            &[],
            Path::new("/tmp/codex.log"),
            AgentKind::CodexCli,
        );
        // Template may or may not use {agent_context_files} depending on
        // user customisation; but {agent_label} should always render the
        // codex label when present in the template. We only assert the
        // substitution was executed without leaving the placeholder behind.
        assert!(!prompt.contains("{agent_label}"));
        assert!(!prompt.contains("{agent_context_files}"));
    }

    #[test]
    fn stage_attachments_copies_every_role() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::new(temp.path().to_path_buf());
        store.init_workspace().unwrap();
        let fb = mk_feedback();
        // Put the referenced attachments into the store.
        store
            .save_attachment(&fb.id, "canvas.png", b"png-bytes")
            .unwrap();
        store
            .save_attachment(&fb.id, "paste-0.png", b"paste-bytes")
            .unwrap();

        let inputs = temp.path().join("job-42").join("inputs");
        let staged = stage_attachments(&store, &fb, &inputs).unwrap();

        // canvas + paste-0 + annotations.json — voice was None, so skipped.
        assert_eq!(staged.len(), 3);
        assert!(inputs.join("canvas.png").exists());
        assert!(inputs.join("paste-0.png").exists());
        assert!(inputs.join("annotations.json").exists());
    }
}
