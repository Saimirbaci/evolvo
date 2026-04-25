//! Per-CLI coding-agent backends.
//!
//! This module isolates every piece of knowledge that differs between the
//! supported agent CLIs (Claude Code, Codex, Gemini, OpenCode) so the rest
//! of the runner can stay agent-agnostic.
//!
//! The selected agent lives on the `LineageJobRecord` so Retry / Resume
//! re-use the same backend that enqueued the work. Spawning goes through
//! [`AgentBackend::build_command`], which sets the correct binary, flags,
//! working directory and environment.
//!
//! Adding a new agent:
//! 1. Add a variant to [`crate::types::AgentKind`].
//! 2. Add a concrete impl of [`AgentBackend`] below.
//! 3. Extend [`backend_for`] to map the new variant.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::collections::HashMap;

use crate::types::{AgentAvailability, AgentKind};

/// Per-CLI spawn contract. Concrete impls live in this module; they are all
/// zero-sized so handing out `Box<dyn AgentBackend>` is cheap.
pub trait AgentBackend: Send + Sync {
    fn id(&self) -> AgentKind;

    /// Binary name looked up on `PATH` (e.g. `"claude"`, `"codex"`).
    fn binary(&self) -> &'static str;

    /// Filename used for the per-job transcript (written to the job's
    /// lineage workspace). Each agent has a different log schema so
    /// keeping distinct filenames lets debuggers tell them apart.
    fn log_filename(&self) -> &'static str;

    /// Project-guide files this agent looks for at the worktree root. The
    /// runner materialises symlinks for the non-Claude backends so each
    /// agent picks up `CLAUDE.md` under its own discovery name.
    fn context_files(&self) -> &'static [&'static str];

    /// Build a fully-wired `Command` ready to be spawned. Callers attach
    /// stdio. The command's `current_dir` is set to `worktree`, `PATH` is
    /// enriched for GUI launches, and auth env vars that would shadow a
    /// subscription login are scrubbed (see per-impl notes).
    fn build_command(&self, prompt: &str, worktree: &Path) -> Command;
}

/// Dispatch helper: one place to map the persisted `AgentKind` to a boxed
/// backend. Clone-friendly because every impl is ZST.
pub fn backend_for(kind: AgentKind) -> Box<dyn AgentBackend> {
    match kind {
        AgentKind::ClaudeCode => Box::new(ClaudeBackend),
        AgentKind::CodexCli => Box::new(CodexBackend),
        AgentKind::GeminiCli => Box::new(GeminiBackend),
        AgentKind::OpenCode => Box::new(OpenCodeBackend),
    }
}

/// PATH prepend list for agent spawns. The host Evolvo may be launched from
/// Finder/Dock on macOS, which inherits a minimal PATH — without this list
/// `which claude` would fail even when the CLI is installed under a common
/// user-local prefix.
fn enriched_path() -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        parts.push(format!("{home}/.cargo/bin"));
        parts.push(format!("{home}/.local/bin"));
        parts.push(format!("{home}/.bun/bin"));
        parts.push(format!("{home}/.opencode/bin"));
        parts.push(format!("{home}/.volta/bin"));
        parts.push(format!("{home}/.nvm/versions/node/current/bin"));
        parts.push(format!("{home}/.npm-global/bin"));
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

/// Claude Code (Anthropic). Streams JSONL tool events to stdout when
/// invoked with `--output-format stream-json --verbose`. We scrub
/// `ANTHROPIC_API_KEY` / `ANTHROPIC_AUTH_TOKEN` to force the subscription
/// auth path, since a zero-balance API key shadows subscription logins.
pub struct ClaudeBackend;

impl AgentBackend for ClaudeBackend {
    fn id(&self) -> AgentKind {
        AgentKind::ClaudeCode
    }
    fn binary(&self) -> &'static str {
        "claude"
    }
    fn log_filename(&self) -> &'static str {
        "claude.log"
    }
    fn context_files(&self) -> &'static [&'static str] {
        &["CLAUDE.md", ".claude/agents", ".claude/rules"]
    }

    fn build_command(&self, prompt: &str, worktree: &Path) -> Command {
        let mut cmd = Command::new(self.binary());
        cmd.arg("-p")
            .arg(prompt)
            .arg("--dangerously-skip-permissions")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--verbose")
            .current_dir(worktree)
            .env("PATH", enriched_path())
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("ANTHROPIC_AUTH_TOKEN")
            .stdin(Stdio::null());
        cmd
    }
}

/// Codex CLI (OpenAI). `codex exec <prompt>` is the non-interactive entry;
/// `--json` emits newline-delimited event objects (schema differs from
/// Claude's). `--dangerously-bypass-approvals-and-sandbox` is the
/// permission-bypass flag the user pre-approves for a lineage run, mirroring
/// the safety envelope `claude --dangerously-skip-permissions` provides.
pub struct CodexBackend;

impl AgentBackend for CodexBackend {
    fn id(&self) -> AgentKind {
        AgentKind::CodexCli
    }
    fn binary(&self) -> &'static str {
        "codex"
    }
    fn log_filename(&self) -> &'static str {
        "codex.log"
    }
    fn context_files(&self) -> &'static [&'static str] {
        &["AGENTS.md"]
    }

    fn build_command(&self, prompt: &str, worktree: &Path) -> Command {
        let mut cmd = Command::new(self.binary());
        cmd.arg("exec")
            .arg("--json")
            .arg("--dangerously-bypass-approvals-and-sandbox")
            .arg(prompt)
            .current_dir(worktree)
            .env("PATH", enriched_path())
            .stdin(Stdio::null());
        cmd
    }
}

/// Gemini CLI (Google). `gemini -p <prompt> --yolo` is the non-interactive
/// form. Output is human-readable by default; unlike Claude/Codex there is
/// no stable JSONL stream, so the log is a plain transcript.
pub struct GeminiBackend;

impl AgentBackend for GeminiBackend {
    fn id(&self) -> AgentKind {
        AgentKind::GeminiCli
    }
    fn binary(&self) -> &'static str {
        "gemini"
    }
    fn log_filename(&self) -> &'static str {
        "gemini.log"
    }
    fn context_files(&self) -> &'static [&'static str] {
        &["GEMINI.md", "AGENTS.md"]
    }

    fn build_command(&self, prompt: &str, worktree: &Path) -> Command {
        let mut cmd = Command::new(self.binary());
        cmd.arg("--yolo")
            .arg("-p")
            .arg(prompt)
            .current_dir(worktree)
            .env("PATH", enriched_path())
            .stdin(Stdio::null());
        cmd
    }
}

/// OpenCode CLI (sst.dev). `opencode run <prompt>` runs a single
/// non-interactive turn; provider/model selection lives in `opencode.json`
/// at the worktree root or via `--model <provider/model>`.
pub struct OpenCodeBackend;

impl AgentBackend for OpenCodeBackend {
    fn id(&self) -> AgentKind {
        AgentKind::OpenCode
    }
    fn binary(&self) -> &'static str {
        "opencode"
    }
    fn log_filename(&self) -> &'static str {
        "opencode.log"
    }
    fn context_files(&self) -> &'static [&'static str] {
        &["AGENTS.md", "opencode.json"]
    }

    fn build_command(&self, prompt: &str, worktree: &Path) -> Command {
        let mut cmd = Command::new(self.binary());
        cmd.arg("run")
            .arg(prompt)
            .current_dir(worktree)
            .env("PATH", enriched_path())
            .stdin(Stdio::null());
        cmd
    }
}

/// Per-process cache of `is_installed` probes. `which` forks, which is
/// cheap but not free — the UI polls availability every time the feedback
/// panel mounts, so caching avoids a visible jitter. The cache is cleared
/// on process restart; if the user installs an agent and wants it to show
/// up immediately, re-launching Evolvo is the documented path.
static INSTALL_CACHE: OnceLock<Mutex<HashMap<AgentKind, bool>>> = OnceLock::new();

/// Best-effort check that the backend binary is on PATH. Walks `PATH`
/// manually so we don't depend on a `which` crate; the list is the same
/// enriched PATH used for spawns.
pub fn is_installed(kind: AgentKind) -> bool {
    let cache = INSTALL_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(guard) = cache.lock() {
        if let Some(&cached) = guard.get(&kind) {
            return cached;
        }
    }
    let binary = backend_for(kind).binary();
    let found = binary_on_path(binary);
    if let Ok(mut guard) = cache.lock() {
        guard.insert(kind, found);
    }
    found
}

fn binary_on_path(binary: &str) -> bool {
    let path = enriched_path();
    for dir in path.split(':') {
        if dir.is_empty() {
            continue;
        }
        let candidate = Path::new(dir).join(binary);
        if candidate.is_file() {
            return true;
        }
    }
    false
}

/// Snapshot availability for every known agent. Used by the
/// `list_available_agents` Tauri command so the UI can grey out chips
/// whose binaries are missing.
pub fn availability() -> Vec<AgentAvailability> {
    AgentKind::all()
        .into_iter()
        .map(|kind| {
            let binary = backend_for(kind).binary().to_string();
            AgentAvailability {
                kind,
                binary,
                installed: is_installed(kind),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn claude_backend_command_has_expected_flags() {
        let worktree = PathBuf::from("/tmp/worktree");
        let cmd = ClaudeBackend.build_command("hello", &worktree);
        assert_eq!(cmd.get_program(), "claude");
        let args: Vec<&str> = cmd
            .get_args()
            .map(|s| s.to_str().unwrap_or(""))
            .collect();
        assert_eq!(
            args,
            vec![
                "-p",
                "hello",
                "--dangerously-skip-permissions",
                "--output-format",
                "stream-json",
                "--verbose",
            ]
        );
    }

    #[test]
    fn codex_backend_uses_exec_json_and_bypass() {
        let cmd = CodexBackend.build_command("hi", Path::new("/tmp/w"));
        assert_eq!(cmd.get_program(), "codex");
        let args: Vec<&str> = cmd
            .get_args()
            .map(|s| s.to_str().unwrap_or(""))
            .collect();
        assert_eq!(
            args,
            vec![
                "exec",
                "--json",
                "--dangerously-bypass-approvals-and-sandbox",
                "hi",
            ]
        );
    }

    #[test]
    fn gemini_backend_uses_yolo_and_prompt() {
        let cmd = GeminiBackend.build_command("hi", Path::new("/tmp/w"));
        assert_eq!(cmd.get_program(), "gemini");
        let args: Vec<&str> = cmd
            .get_args()
            .map(|s| s.to_str().unwrap_or(""))
            .collect();
        assert_eq!(args, vec!["--yolo", "-p", "hi"]);
    }

    #[test]
    fn opencode_backend_uses_run_subcommand() {
        let cmd = OpenCodeBackend.build_command("hi", Path::new("/tmp/w"));
        assert_eq!(cmd.get_program(), "opencode");
        let args: Vec<&str> = cmd
            .get_args()
            .map(|s| s.to_str().unwrap_or(""))
            .collect();
        assert_eq!(args, vec!["run", "hi"]);
    }

    #[test]
    fn backend_for_maps_every_variant() {
        for kind in AgentKind::all() {
            let b = backend_for(kind);
            assert_eq!(b.id(), kind);
            assert!(!b.binary().is_empty());
            assert!(!b.log_filename().is_empty());
        }
    }

    #[test]
    fn availability_returns_one_entry_per_variant() {
        let list = availability();
        assert_eq!(list.len(), AgentKind::all().len());
        for kind in AgentKind::all() {
            assert!(list.iter().any(|a| a.kind == kind));
        }
    }

    #[test]
    fn is_installed_returns_false_for_unknown_binary() {
        // The cache is keyed per AgentKind, so this doesn't test a real
        // variant being absent — it just exercises the PATH walker.
        assert!(!binary_on_path("there-is-no-binary-called-this-xyz123"));
    }
}
