//! Per-CLI coding-agent backends.
//!
//! This module isolates every piece of knowledge that differs between the
//! supported agent CLIs (Claude Code, Codex, Gemini, Forge) so the rest
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

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::types::{AgentAvailability, AgentKind, LocalDelegateAvailability};

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
        AgentKind::Forge => Box::new(ForgeBackend),
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
        parts.push(format!("{home}/.forge/bin"));
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
        // `--skip-trust` suppresses the interactive "do you trust this
        // workspace?" prompt that gemini-cli puts up the first time it
        // runs in a directory it hasn't seen before. The lineage worktree
        // is always a fresh path, so without this flag every run blocks on
        // a TTY prompt and `--yolo` alone isn't enough — `--yolo` only
        // disables tool-confirmation, not workspace trust.
        cmd.arg("--yolo")
            .arg("--skip-trust")
            .arg("-p")
            .arg(prompt)
            .current_dir(worktree)
            .env("PATH", enriched_path())
            .stdin(Stdio::null());
        cmd
    }
}

/// ForgeCode (forgecode.dev). `forge -p <prompt>` runs a single
/// non-interactive turn ("One-Shot CLI Mode" in the upstream README).
/// Provider / model selection lives in `forge.yaml` at the worktree root
/// or via `forge provider login` once per host. Forge picks up `AGENTS.md`
/// as its project guide, the same convention Codex uses, so the runner's
/// existing `AGENTS.md → CLAUDE.md` symlink continues to wire up the right
/// context for free.
pub struct ForgeBackend;

impl AgentBackend for ForgeBackend {
    fn id(&self) -> AgentKind {
        AgentKind::Forge
    }
    fn binary(&self) -> &'static str {
        "forge"
    }
    fn log_filename(&self) -> &'static str {
        "forge.log"
    }
    fn context_files(&self) -> &'static [&'static str] {
        &["AGENTS.md", "forge.yaml"]
    }

    fn build_command(&self, prompt: &str, worktree: &Path) -> Command {
        let mut cmd = Command::new(self.binary());
        cmd.arg("-p")
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

/// Probe whether the local-delegation worker (`pi` CLI + a reachable
/// Ollama server at `127.0.0.1:11434`) is available. The result is cached
/// per process; users who install `pi` or start Ollama after launching
/// Evolvo need to relaunch — same convention as the per-agent probe.
///
/// Both pieces must be true for delegation to make sense: `pi` is the
/// invocation surface the supervisor talks to, Ollama is what `pi` calls
/// for inference. We do not parse `~/.pi/agent/models.json` here — if pi
/// is installed and Ollama is reachable, we trust the user's pi config to
/// have a usable model. The UI shows a clear error if delegation later
/// fails at run time.
pub fn local_delegate_availability() -> LocalDelegateAvailability {
    static CACHE: OnceLock<LocalDelegateAvailability> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            let pi_installed = binary_on_path("pi");
            let ollama_reachable = ollama_health_check();
            LocalDelegateAvailability {
                pi_installed,
                ollama_reachable,
                provider: "ollama".into(),
                default_model: "gemma4".into(),
            }
        })
        .clone()
}

/// Best-effort HEAD-style probe of the Ollama API root. We open a TCP
/// connection to `127.0.0.1:11434` with a 250ms connect timeout and write
/// a minimal HTTP/1.1 GET against `/`. Any 2xx/3xx/4xx response counts as
/// "Ollama is up" — even a 404 means the server answered. We avoid taking
/// a `reqwest` dependency just for one health check.
fn ollama_health_check() -> bool {
    let addr: SocketAddr = match "127.0.0.1:11434".parse() {
        Ok(a) => a,
        Err(_) => return false,
    };
    let mut stream = match TcpStream::connect_timeout(&addr, Duration::from_millis(250)) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(250)));
    let req = b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";
    if stream.write_all(req).is_err() {
        return false;
    }
    let mut buf = [0u8; 12];
    match stream.read(&mut buf) {
        Ok(n) if n >= 12 => buf.starts_with(b"HTTP/"),
        _ => false,
    }
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
        let args: Vec<&str> = cmd.get_args().map(|s| s.to_str().unwrap_or("")).collect();
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
        let args: Vec<&str> = cmd.get_args().map(|s| s.to_str().unwrap_or("")).collect();
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
        let args: Vec<&str> = cmd.get_args().map(|s| s.to_str().unwrap_or("")).collect();
        assert_eq!(args, vec!["--yolo", "--skip-trust", "-p", "hi"]);
    }

    #[test]
    fn forge_backend_uses_prompt_flag() {
        let cmd = ForgeBackend.build_command("hi", Path::new("/tmp/w"));
        assert_eq!(cmd.get_program(), "forge");
        let args: Vec<&str> = cmd.get_args().map(|s| s.to_str().unwrap_or("")).collect();
        assert_eq!(args, vec!["-p", "hi"]);
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
    fn ollama_health_check_does_not_panic_when_offline() {
        // The probe must always return a bool quickly — even if no Ollama
        // server is running on the dev machine. We don't assert the value
        // because CI machines could legitimately have one running.
        let _ = ollama_health_check();
    }

    #[test]
    fn local_delegate_availability_returns_known_provider() {
        let a = local_delegate_availability();
        assert_eq!(a.provider, "ollama");
        assert_eq!(a.default_model, "gemma4");
    }

    #[test]
    fn is_installed_returns_false_for_unknown_binary() {
        // The cache is keyed per AgentKind, so this doesn't test a real
        // variant being absent — it just exercises the PATH walker.
        assert!(!binary_on_path("there-is-no-binary-called-this-xyz123"));
    }
}
