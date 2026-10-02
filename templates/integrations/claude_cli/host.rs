//! Local LLM engine backed by the user's `claude` CLI (Claude Code).
//!
//! Use this for any behaviour where the app's own logic needs a model:
//! agents doing work, assistants replying, summaries, classification,
//! generated content. No API key — it uses whatever account the user's
//! `claude` CLI is logged into.
//!
//! Design:
//! - [`LlmEngine`] is the seam. Hold an `Arc<dyn LlmEngine>` in app state;
//!   production uses [`ClaudeCli`], tests use [`ScriptedEngine`] so
//!   `cargo test` never spawns a real model call.
//! - Calls are **blocking** (seconds to minutes). Run them from a
//!   background thread (`std::thread::spawn`) or
//!   `tauri::async_runtime::spawn_blocking`, never directly in a sync
//!   `#[tauri::command]` (that would freeze the window).
//! - The CLI runs with **no tools** (`--tools ""`): it can only read the
//!   prompt you send and answer. It cannot touch the filesystem.
//! - Prompt goes in via stdin (no ARG_MAX limits), output via a temp file
//!   (no pipe deadlock), whole process group killed on timeout.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

/// One model call.
#[derive(Debug, Clone)]
pub struct LlmRequest {
    /// System prompt: who the model is in this app (e.g. the agent's role).
    pub system: String,
    /// The user/turn content.
    pub prompt: String,
    /// `None` = CLI default. Aliases like `"sonnet"`, `"opus"`, `"haiku"` work.
    pub model: Option<String>,
    /// When set, the reply is forced to match this JSON Schema and comes
    /// back parsed in [`LlmReply::structured`].
    pub json_schema: Option<Value>,
    pub timeout: Duration,
}

impl LlmRequest {
    pub fn new(system: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            system: system.into(),
            prompt: prompt.into(),
            model: None,
            json_schema: None,
            timeout: Duration::from_secs(180),
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn with_schema(mut self, schema: Value) -> Self {
        self.json_schema = Some(schema);
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LlmReply {
    /// Plain-text answer (empty when only structured output was requested).
    pub text: String,
    /// Parsed structured output when `json_schema` was set.
    pub structured: Option<Value>,
    pub duration_ms: u64,
}

pub trait LlmEngine: Send + Sync {
    fn complete(&self, req: &LlmRequest) -> Result<LlmReply, String>;
}

/// Production engine: shells out to `claude -p`.
#[derive(Debug, Default, Clone)]
pub struct ClaudeCli;

impl LlmEngine for ClaudeCli {
    fn complete(&self, req: &LlmRequest) -> Result<LlmReply, String> {
        let started = Instant::now();
        let stdout = run_claude(req)?;
        let (text, structured) = parse_envelope(&stdout)?;
        Ok(LlmReply {
            text,
            structured,
            duration_ms: started.elapsed().as_millis() as u64,
        })
    }
}

/// Test engine: returns queued replies in order (or a fixed fallback) and
/// records every request so tests can assert on prompts.
#[derive(Default)]
pub struct ScriptedEngine {
    replies: Mutex<Vec<Result<LlmReply, String>>>,
    fallback: Option<String>,
    pub requests: Mutex<Vec<LlmRequest>>,
}

impl ScriptedEngine {
    /// Always answer with `text`.
    pub fn always(text: impl Into<String>) -> Self {
        Self {
            fallback: Some(text.into()),
            ..Default::default()
        }
    }

    /// Answer with these results in order, then fail.
    pub fn sequence(replies: Vec<Result<String, String>>) -> Self {
        let mut queued: Vec<Result<LlmReply, String>> = replies
            .into_iter()
            .map(|r| {
                r.map(|text| LlmReply {
                    text,
                    structured: None,
                    duration_ms: 0,
                })
            })
            .collect();
        queued.reverse();
        Self {
            replies: Mutex::new(queued),
            ..Default::default()
        }
    }

    pub fn request_count(&self) -> usize {
        self.requests.lock().map(|r| r.len()).unwrap_or(0)
    }
}

impl LlmEngine for ScriptedEngine {
    fn complete(&self, req: &LlmRequest) -> Result<LlmReply, String> {
        if let Ok(mut log) = self.requests.lock() {
            log.push(req.clone());
        }
        if let Ok(mut queued) = self.replies.lock() {
            if let Some(next) = queued.pop() {
                return next;
            }
        }
        match &self.fallback {
            Some(text) => Ok(LlmReply {
                text: text.clone(),
                structured: serde_json::from_str(text).ok(),
                duration_ms: 0,
            }),
            None => Err("ScriptedEngine: no scripted reply left".into()),
        }
    }
}

/// Whether the `claude` CLI is installed. (Login state is only known once
/// a call is made — surface call errors in the UI.)
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeCliStatus {
    pub installed: bool,
    pub hint: String,
}

pub fn status() -> ClaudeCliStatus {
    let installed = find_binary("claude");
    ClaudeCliStatus {
        installed,
        hint: if installed {
            "claude CLI found".into()
        } else {
            "Install Claude Code (`npm i -g @anthropic-ai/claude-code`) and run `claude` once to log in.".into()
        },
    }
}

/// Tauri command so the UI can show a setup hint before the first call.
/// Register it in `invoke_handler!` and mirror it in `interop.rs`.
#[tauri::command]
pub fn claude_cli_status() -> ClaudeCliStatus {
    status()
}

/// Extract (text, structured) from `claude -p --output-format json`.
pub fn parse_envelope(stdout: &str) -> Result<(String, Option<Value>), String> {
    let envelope: Value = serde_json::from_str(stdout.trim())
        .map_err(|e| format!("model returned non-JSON output: {e}"))?;
    if envelope.get("is_error").and_then(Value::as_bool) == Some(true) {
        let msg = envelope
            .get("result")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        return Err(format!("model call failed: {}", msg.chars().take(300).collect::<String>()));
    }
    let text = envelope
        .get("result")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let structured = envelope.get("structured_output").cloned().filter(|v| !v.is_null());
    if text.trim().is_empty() && structured.is_none() {
        return Err("model returned an empty reply".into());
    }
    Ok((text, structured))
}

fn enriched_path() -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        for dir in [".local/bin", ".npm-global/bin", ".bun/bin", ".volta/bin", ".nvm/versions/node/current/bin"] {
            parts.push(format!("{home}/{dir}"));
        }
    }
    parts.push("/opt/homebrew/bin".into());
    parts.push("/usr/local/bin".into());
    if let Ok(existing) = std::env::var("PATH") {
        parts.push(existing);
    }
    parts.push("/usr/bin:/bin".into());
    parts.join(":")
}

fn find_binary(name: &str) -> bool {
    enriched_path()
        .split(':')
        .filter(|d| !d.is_empty())
        .any(|d| std::path::Path::new(d).join(name).is_file())
}

fn run_claude(req: &LlmRequest) -> Result<String, String> {
    let tmp_err = |e: std::io::Error| format!("temp file error: {e}");
    let mut stdin_file = tempfile::NamedTempFile::new().map_err(tmp_err)?;
    stdin_file.write_all(req.prompt.as_bytes()).map_err(tmp_err)?;
    let stdout_file = tempfile::NamedTempFile::new().map_err(tmp_err)?;

    let mut cmd = Command::new("claude");
    cmd.arg("-p")
        .arg("--output-format")
        .arg("json")
        .arg("--tools")
        .arg("")
        .arg("--system-prompt")
        .arg(&req.system);
    if let Some(model) = &req.model {
        cmd.arg("--model").arg(model);
    }
    if let Some(schema) = &req.json_schema {
        cmd.arg("--json-schema").arg(schema.to_string());
    }
    cmd.env("PATH", enriched_path())
        // Prefer the CLI's logged-in subscription over a possibly-empty key.
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .stdin(Stdio::from(stdin_file.reopen().map_err(tmp_err)?))
        .stdout(Stdio::from(stdout_file.reopen().map_err(tmp_err)?))
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            "The `claude` CLI is not installed or not on PATH.".to_string()
        } else {
            format!("could not start `claude`: {e}")
        }
    })?;
    let deadline = Instant::now() + req.timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() >= deadline => {
                #[cfg(unix)]
                {
                    let _ = Command::new("kill")
                        .arg("-9")
                        .arg(format!("-{}", child.id()))
                        .status();
                }
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("model call timed out after {}s", req.timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(200)),
            Err(e) => return Err(format!("waiting for `claude`: {e}")),
        }
    }
    std::fs::read_to_string(stdout_file.path()).map_err(|e| format!("read model output: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_text_and_structured_replies() {
        let (text, structured) =
            parse_envelope(&json!({"is_error": false, "result": "hi"}).to_string()).unwrap();
        assert_eq!(text, "hi");
        assert!(structured.is_none());
        let (_, structured) = parse_envelope(
            &json!({"is_error": false, "result": "", "structured_output": {"ok": true}}).to_string(),
        )
        .unwrap();
        assert_eq!(structured, Some(json!({"ok": true})));
    }

    #[test]
    fn surfaces_cli_errors_and_empty_replies() {
        let err = parse_envelope(r#"{"is_error": true, "result": "Not logged in"}"#).unwrap_err();
        assert!(err.contains("Not logged in"));
        assert!(parse_envelope(r#"{"is_error": false, "result": ""}"#).is_err());
        assert!(parse_envelope("garbage").is_err());
    }

    #[test]
    fn scripted_engine_replays_in_order_and_records_requests() {
        let engine = ScriptedEngine::sequence(vec![Ok("one".into()), Err("boom".into())]);
        let req = LlmRequest::new("sys", "hello");
        assert_eq!(engine.complete(&req).unwrap().text, "one");
        assert_eq!(engine.complete(&req).unwrap_err(), "boom");
        assert!(engine.complete(&req).is_err());
        assert_eq!(engine.request_count(), 3);
        let always = ScriptedEngine::always(r#"{"x": 1}"#);
        assert_eq!(always.complete(&req).unwrap().structured, Some(json!({"x": 1})));
    }
}
