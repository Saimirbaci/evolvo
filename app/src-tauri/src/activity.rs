//! Live agent activity — what the agent is doing "under the hood".
//!
//! Turns a running job's agent transcript into a short, human-readable
//! feed ("Reading crates/zed/src/zed.rs", "Running cargo check", the
//! agent's own one-line updates), a coarse phase (finding repo → exploring
//! → implementing → verifying → build checks → review), and the list of
//! files changed in the worktree. Purely derived from files already on
//! disk — no extra model calls, nothing written.
//!
//! The UI re-reads this whenever the workspace watcher reports a log change
//! for the job (`lineage://artifacts-changed`, debounced), so the feed is
//! live without polling.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::store::Store;
use crate::types::{FeedbackRecord, LineageJobRecord, LineageJobStatus};

/// Bytes read from the end of the transcript. Claude stream-json logs grow
/// to tens of MB on long runs; the feed only needs the recent tail.
const TAIL_BYTES: u64 = 768 * 1024;
const MAX_EVENTS: usize = 60;
const MAX_CHANGED_FILES: usize = 200;
const MAX_TEXT: usize = 220;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEvent {
    /// `"tool"`, `"message"` (the agent's own words), `"result"` or
    /// `"output"` (plain-text transcripts).
    pub kind: String,
    /// Tool name for `kind == "tool"` (Read, Edit, Bash, …).
    #[serde(default)]
    pub tool: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PhaseStep {
    pub key: String,
    pub label: String,
    /// `"done"`, `"active"`, `"failed"` or `"pending"`.
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct JobActivity {
    pub job_id: String,
    /// Key of the current phase (see `PHASES`).
    pub phase: String,
    pub phases: Vec<PhaseStep>,
    /// The most recent action, for one-line "now: …" displays.
    pub current: Option<String>,
    /// Oldest first, at most `MAX_EVENTS`.
    pub events: Vec<ActivityEvent>,
    pub changed_files: Vec<String>,
    #[serde(default)]
    pub started_at_unix_ms: Option<u64>,
    #[serde(default)]
    pub log_updated_at_unix_ms: Option<u64>,
    /// From the transcript's final `result` line, once the agent exits.
    #[serde(default)]
    pub cost_usd: Option<f64>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    pub running: bool,
}

const PHASES: &[(&str, &str)] = &[
    ("repo", "Find source"),
    ("exploring", "Exploring"),
    ("implementing", "Implementing"),
    ("verifying", "Verifying"),
    ("checks", "Build checks"),
    ("review", "Ready for review"),
];

// ---------------------------------------------------------------------
// Transcript parsing
// ---------------------------------------------------------------------

/// Parse transcript lines into feed events plus the final result (cost,
/// duration) when present. Understands Claude Code stream-json and Codex
/// `exec --json`; any other line becomes plain `output`.
pub fn parse_transcript(text: &str) -> (Vec<ActivityEvent>, Option<(f64, u64)>) {
    let mut events = Vec::new();
    let mut result = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            if !line.starts_with('{') {
                events.push(ActivityEvent {
                    kind: "output".into(),
                    tool: None,
                    text: truncate(line, MAX_TEXT),
                });
            }
            continue;
        };
        match v.get("type").and_then(Value::as_str) {
            Some("assistant") => {
                let Some(content) = v.pointer("/message/content").and_then(Value::as_array) else {
                    continue;
                };
                for item in content {
                    match item.get("type").and_then(Value::as_str) {
                        Some("tool_use") => {
                            let name = item.get("name").and_then(Value::as_str).unwrap_or("tool");
                            let input = item.get("input").cloned().unwrap_or(Value::Null);
                            events.push(ActivityEvent {
                                kind: "tool".into(),
                                tool: Some(name.to_string()),
                                text: describe_tool(name, &input),
                            });
                        }
                        Some("text") => {
                            let t = item
                                .get("text")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .trim();
                            if !t.is_empty() && !t.starts_with("STAGE_RESULT") {
                                events.push(ActivityEvent {
                                    kind: "message".into(),
                                    tool: None,
                                    text: first_sentence(t),
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some("result") => {
                let cost = v
                    .get("total_cost_usd")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                let dur = v.get("duration_ms").and_then(Value::as_u64).unwrap_or(0);
                result = Some((cost, dur));
                let failed = v.get("is_error").and_then(Value::as_bool) == Some(true);
                events.push(ActivityEvent {
                    kind: "result".into(),
                    tool: None,
                    text: if failed {
                        "Agent session ended with an error".into()
                    } else {
                        "Agent finished its session".into()
                    },
                });
            }
            _ => {
                if let Some(e) = codex_event(&v) {
                    events.push(e);
                }
            }
        }
    }
    (events, result)
}

/// Best-effort Codex `exec --json` support (both the `item.*` and the older
/// `msg` shapes).
fn codex_event(v: &Value) -> Option<ActivityEvent> {
    let item = v.get("item").or_else(|| v.get("msg"))?;
    let kind = item.get("type").and_then(Value::as_str)?;
    match kind {
        "command_execution" | "exec_command_begin" => {
            let cmd = match item.get("command") {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Array(a)) => a
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" "),
                _ => return None,
            };
            Some(ActivityEvent {
                kind: "tool".into(),
                tool: Some("Bash".into()),
                text: format!("Running {}", clean_command(&cmd)),
            })
        }
        "file_change" | "patch_apply_begin" => Some(ActivityEvent {
            kind: "tool".into(),
            tool: Some("Edit".into()),
            text: "Editing files".into(),
        }),
        "agent_message" => {
            let t = item
                .get("text")
                .or_else(|| item.get("message"))
                .and_then(Value::as_str)?;
            Some(ActivityEvent {
                kind: "message".into(),
                tool: None,
                text: first_sentence(t),
            })
        }
        _ => None,
    }
}

fn describe_tool(name: &str, input: &Value) -> String {
    let s = |k: &str| {
        input
            .get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    match name {
        "Read" | "NotebookRead" => format!("Reading {}", rel_path(&s("file_path"))),
        "Edit" | "MultiEdit" | "NotebookEdit" => format!("Editing {}", rel_path(&s("file_path"))),
        "Write" => format!("Writing {}", rel_path(&s("file_path"))),
        "Bash" => format!("Running {}", clean_command(&s("command"))),
        "Grep" => format!("Searching code for “{}”", truncate(&s("pattern"), 80)),
        "Glob" => format!("Listing files {}", truncate(&s("pattern"), 80)),
        "WebSearch" => format!("Searching the web: {}", truncate(&s("query"), 100)),
        "WebFetch" => format!("Reading {}", truncate(&s("url"), 100)),
        "Skill" => format!("Loading skill {}", s("skill")),
        "Task" | "Agent" => format!(
            "Starting a helper agent: {}",
            truncate(&s("description"), 100)
        ),
        "TodoWrite" => "Updating its task list".into(),
        "ToolSearch" => "Looking up tools".into(),
        other => format!("Using {other}"),
    }
}

/// Paths are shown relative to the worktree — the absolute prefix is noise.
fn rel_path(p: &str) -> String {
    match p.find("/worktree/") {
        Some(i) => p[i + "/worktree/".len()..].to_string(),
        None => p.to_string(),
    }
}

/// Drop the `cd "<worktree>" && ` preamble agents prefix every call with,
/// flatten whitespace, and cut to one readable line.
fn clean_command(cmd: &str) -> String {
    let mut c = cmd.trim();
    if let Some(rest) = c.strip_prefix("cd ") {
        if let Some(i) = rest.find("&&") {
            c = rest[i + 2..].trim();
        }
    }
    let flat: String = c.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate(&flat, 140)
}

fn first_sentence(t: &str) -> String {
    let line = t
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    let line = line.trim_start_matches(['#', '*', '-', ' ']);
    truncate(line, MAX_TEXT)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn is_edit(e: &ActivityEvent) -> bool {
    matches!(
        e.tool.as_deref(),
        Some("Edit" | "MultiEdit" | "Write" | "NotebookEdit")
    )
}

/// Build/test/lint commands — running one after editing means verifying.
fn is_verification(e: &ActivityEvent) -> bool {
    if e.tool.as_deref() != Some("Bash") {
        return false;
    }
    const MARKERS: &[&str] = &[
        "cargo check",
        "cargo test",
        "cargo build",
        "cargo clippy",
        "cargo nextest",
        "script/clippy",
        "npm test",
        "npm run build",
        "npm run test",
        "pnpm test",
        "pnpm run",
        "yarn test",
        "yarn build",
        "go test",
        "go build",
        "pytest",
        "make test",
        "tsc",
    ];
    MARKERS.iter().any(|m| e.text.contains(m))
}

// ---------------------------------------------------------------------
// Phase
// ---------------------------------------------------------------------

/// Current phase key from job state, notes and the recent events.
pub fn derive_phase(
    job: &LineageJobRecord,
    feedback: Option<&FeedbackRecord>,
    events: &[ActivityEvent],
    has_changes: bool,
) -> String {
    let resolving_repo = feedback
        .and_then(|f| f.target_app.as_ref())
        .is_some_and(|t| t.repo_path.is_none());
    let last_start = job
        .notes
        .iter()
        .rposition(|n| n.contains(" starting (") || n.contains("retry requested"));
    let gates_running = job
        .notes
        .iter()
        .rposition(|n| n.contains("running build gates"))
        .is_some_and(|i| last_start.is_none_or(|s| i > s));
    let work_phase = || {
        let last_edit = events.iter().rposition(is_edit);
        match last_edit {
            None if !has_changes => "exploring",
            Some(i) if events[i..].iter().any(is_verification) => "verifying",
            _ => "implementing",
        }
        .to_string()
    };
    match job.status {
        LineageJobStatus::Pending if resolving_repo => "repo".into(),
        LineageJobStatus::Pending => "queued".into(),
        LineageJobStatus::Implementing | LineageJobStatus::Triaging if gates_running => {
            "checks".into()
        }
        LineageJobStatus::Implementing | LineageJobStatus::Triaging => work_phase(),
        LineageJobStatus::Planned => "plan_review".into(),
        LineageJobStatus::BuildReady => "review".into(),
        LineageJobStatus::Merging => "merging".into(),
        LineageJobStatus::Promoted => "merged".into(),
        LineageJobStatus::Rejected => "rejected".into(),
        LineageJobStatus::Failed => {
            if gates_running {
                "checks".into()
            } else if last_start.is_none() && resolving_repo {
                "repo".into()
            } else {
                work_phase()
            }
        }
    }
}

fn phase_steps(phase: &str, failed: bool, external: bool) -> Vec<PhaseStep> {
    let order: Vec<&(&str, &str)> = PHASES
        .iter()
        .filter(|(k, _)| external || *k != "repo")
        .collect();
    // Phases outside the stepper map onto the nearest step.
    let anchor = match phase {
        "queued" => {
            if external {
                "repo"
            } else {
                "exploring"
            }
        }
        "plan_review" => "exploring",
        "merging" | "merged" | "rejected" => "review",
        other => other,
    };
    let current = order.iter().position(|(k, _)| *k == anchor);
    let all_done = matches!(phase, "merged" | "merging");
    order
        .iter()
        .enumerate()
        .map(|(i, (key, label))| {
            let state = match current {
                _ if all_done => "done",
                Some(c) if i < c => "done",
                Some(c) if i == c && failed => "failed",
                Some(c) if i == c && phase == "review" => "done",
                Some(c) if i == c && phase == "queued" => "pending",
                Some(c) if i == c => "active",
                _ => "pending",
            };
            PhaseStep {
                key: key.to_string(),
                label: label.to_string(),
                state: state.to_string(),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------
// Assembly
// ---------------------------------------------------------------------

/// The transcript the agent is (or was last) writing: the most recently
/// modified of the job's main log and any per-stage logs.
fn active_log(job: &LineageJobRecord, job_dir: &Path) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = job.log_path.iter().map(PathBuf::from).collect();
    if let Ok(entries) = std::fs::read_dir(job_dir.join("logs")) {
        candidates.extend(
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "log")),
        );
    }
    candidates
        .into_iter()
        .filter_map(|p| {
            let modified = std::fs::metadata(&p).and_then(|m| m.modified()).ok()?;
            Some((modified, p))
        })
        .max_by_key(|(m, _)| *m)
        .map(|(_, p)| p)
}

fn read_tail(path: &Path) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return String::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(TAIL_BYTES);
    if f.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut buf = Vec::new();
    let _ = f.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf).to_string();
    // Drop the partial first line when we started mid-file.
    if start > 0 {
        text.split_once('\n')
            .map(|(_, rest)| rest.to_string())
            .unwrap_or_default()
    } else {
        text
    }
}

fn mtime_ms(p: &Path) -> Option<u64> {
    let t = std::fs::metadata(p).and_then(|m| m.modified()).ok()?;
    t.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as u64)
}

fn git_lines(dir: &Path, args: &[&str]) -> Vec<String> {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(str::to_string)
                .filter(|l| !l.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Files the run changed: committed on the lineage branch since it forked
/// from `base_branch`, plus uncommitted and untracked files.
pub fn changed_files(worktree: &Path, base_branch: Option<&str>) -> Vec<String> {
    if !worktree.is_dir() {
        return Vec::new();
    }
    let base = base_branch
        .and_then(|b| {
            git_lines(worktree, &["merge-base", "HEAD", b])
                .into_iter()
                .next()
        })
        .unwrap_or_else(|| "HEAD".to_string());
    let mut files = git_lines(worktree, &["diff", "--name-only", &base]);
    for f in git_lines(worktree, &["ls-files", "--others", "--exclude-standard"]) {
        if !files.contains(&f) {
            files.push(f);
        }
    }
    files.truncate(MAX_CHANGED_FILES);
    files
}

pub fn job_activity(store: &Store, job_id: &str) -> Result<JobActivity, String> {
    let job = store
        .load_lineage_job(job_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("lineage job not found: {job_id}"))?;
    let feedback = store.load_feedback(&job.feedback_id).ok().flatten();
    let job_dir = crate::runner::job_workspace_dir(store.layout().root(), &job.id);

    let log = active_log(&job, &job_dir);
    let (mut events, result) = log
        .as_deref()
        .map(|p| parse_transcript(&read_tail(p)))
        .unwrap_or_default();
    if events.len() > MAX_EVENTS {
        events.drain(..events.len() - MAX_EVENTS);
    }

    let changed = job
        .worktree_path
        .as_deref()
        .map(|w| changed_files(Path::new(w), job.base_branch.as_deref()))
        .unwrap_or_default();

    let phase = derive_phase(&job, feedback.as_ref(), &events, !changed.is_empty());
    let external = feedback.as_ref().is_some_and(|f| f.target_app.is_some());
    let failed = job.status == LineageJobStatus::Failed;
    let running = matches!(
        job.status,
        LineageJobStatus::Implementing | LineageJobStatus::Triaging | LineageJobStatus::Merging
    ) || (phase == "repo" && !failed);

    let started_at = std::fs::read(crate::runner::metadata_path(store.layout().root(), &job.id))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.get("started_at_unix_ms").and_then(Value::as_u64));

    let current = if phase == "checks" {
        Some("Evolvo is running the build checks".to_string())
    } else if phase == "repo" {
        job.notes
            .iter()
            .rev()
            .find(|n| n.starts_with("source repo:"))
            .map(|n| n.trim_start_matches("source repo:").trim().to_string())
            .or_else(|| Some("Finding the app's source code".to_string()))
    } else {
        events
            .iter()
            .rev()
            .find(|e| e.kind == "tool" || e.kind == "message")
            .map(|e| e.text.clone())
    };

    Ok(JobActivity {
        job_id: job.id.clone(),
        phases: phase_steps(&phase, failed, external),
        phase,
        current,
        events,
        changed_files: changed,
        started_at_unix_ms: started_at,
        log_updated_at_unix_ms: log.as_deref().and_then(mtime_ms),
        cost_usd: result.map(|(c, _)| c),
        duration_ms: result.map(|(_, d)| d),
        running,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(status: LineageJobStatus, notes: &[&str]) -> LineageJobRecord {
        let mut j: LineageJobRecord = serde_json::from_value(serde_json::json!({
            "id": "job-1", "feedbackId": "fb-1", "status": "pending",
            "title": "t", "summary": "s", "createdAtUnixMs": 1, "updatedAtUnixMs": 1
        }))
        .unwrap();
        j.status = status;
        j.notes = notes.iter().map(|n| n.to_string()).collect();
        j
    }

    fn tool(name: &str, text: &str) -> ActivityEvent {
        ActivityEvent {
            kind: "tool".into(),
            tool: Some(name.into()),
            text: text.into(),
        }
    }

    #[test]
    fn parses_claude_stream_json_into_readable_events() {
        let log = [
            r#"{"type":"system","subtype":"init"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"/ws/lineage_workspaces/job-1/worktree/crates/zed/src/zed.rs"}}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"cd \"/ws/job-1/worktree\" && cargo   check -p zed"}}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Now the status bar component.\nMore detail here."}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"STAGE_RESULT: ok"}]}}"#,
            r#"{"type":"result","total_cost_usd":4.2,"duration_ms":1380000,"is_error":false}"#,
        ]
        .join("\n");
        let (events, result) = parse_transcript(&log);
        let texts: Vec<&str> = events.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(
            texts,
            vec![
                "Reading crates/zed/src/zed.rs",
                "Running cargo check -p zed",
                "Now the status bar component.",
                "Agent finished its session",
            ]
        );
        assert_eq!(result, Some((4.2, 1_380_000)));
    }

    #[test]
    fn parses_codex_and_plain_text_lines() {
        let log = [
            r#"{"type":"item.completed","item":{"type":"command_execution","command":"bash -lc 'go test ./...'"}}"#,
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"Done with the change."}}"#,
            "plain gemini output line",
        ]
        .join("\n");
        let (events, _) = parse_transcript(&log);
        assert_eq!(events[0].text, "Running bash -lc 'go test ./...'");
        assert_eq!(events[1].kind, "message");
        assert_eq!(events[2].kind, "output");
    }

    #[test]
    fn phase_follows_the_work() {
        let running = job(
            LineageJobStatus::Implementing,
            &["Claude Code starting (claude) in worktree"],
        );
        assert_eq!(
            derive_phase(&running, None, &[tool("Read", "Reading a")], false),
            "exploring"
        );
        let edits = [tool("Read", "Reading a"), tool("Edit", "Editing a")];
        assert_eq!(derive_phase(&running, None, &edits, true), "implementing");
        let verify = [
            tool("Edit", "Editing a"),
            tool("Bash", "Running cargo check -p x"),
        ];
        assert_eq!(derive_phase(&running, None, &verify, true), "verifying");
        // Changes on disk but the edit scrolled out of the tail → implementing.
        assert_eq!(
            derive_phase(&running, None, &[tool("Read", "r")], true),
            "implementing"
        );

        let gates = job(
            LineageJobStatus::Implementing,
            &[
                "Claude Code starting (claude)",
                "Claude Code completed — running build gates before marking build ready",
            ],
        );
        assert_eq!(derive_phase(&gates, None, &verify, true), "checks");
        assert_eq!(
            derive_phase(&job(LineageJobStatus::BuildReady, &[]), None, &[], true),
            "review"
        );
    }

    #[test]
    fn external_pending_job_is_finding_its_repo() {
        let mut fb: FeedbackRecord = serde_json::from_value(serde_json::json!({
            "id": "fb-1", "feedbackType": "bug", "status": "new", "pageRoute": "/",
            "feedbackText": "x", "screenshotFilename": null, "voiceFilename": null,
            "voiceTranscript": null, "windowWidth": 1, "windowHeight": 1,
            "createdAtUnixMs": 1, "updatedAtUnixMs": 1, "lineageJobId": null
        }))
        .unwrap();
        fb.target_app = Some(crate::types::TargetApp {
            name: "Zed".into(),
            ..Default::default()
        });
        let pending = job(LineageJobStatus::Pending, &[]);
        assert_eq!(derive_phase(&pending, Some(&fb), &[], false), "repo");
        let steps = phase_steps("repo", false, true);
        assert_eq!(steps[0].state, "active");
        assert_eq!(steps.len(), 6);
        assert_eq!(phase_steps("exploring", false, false).len(), 5);
    }

    #[test]
    fn phase_steps_mark_done_active_and_failed() {
        let s = phase_steps("verifying", false, true);
        let states: Vec<&str> = s.iter().map(|p| p.state.as_str()).collect();
        assert_eq!(
            states,
            vec!["done", "done", "done", "active", "pending", "pending"]
        );
        let f = phase_steps("checks", true, false);
        assert_eq!(
            f.iter().find(|p| p.key == "checks").unwrap().state,
            "failed"
        );
        let r = phase_steps("review", false, false);
        assert!(r.iter().all(|p| p.state == "done"));
    }

    #[test]
    fn clean_command_strips_cd_preamble_and_truncates() {
        assert_eq!(clean_command("cd \"/a b/worktree\" && ls -la"), "ls -la");
        assert_eq!(clean_command("echo   hi\n  there"), "echo hi there");
        assert!(clean_command(&"x".repeat(500)).chars().count() <= 140);
    }

    #[test]
    fn changed_files_include_committed_uncommitted_and_untracked() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let git = |args: &[&str]| {
            assert!(Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(args)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .status()
                .unwrap()
                .success());
        };
        git(&["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "1").unwrap();
        std::fs::write(repo.join("b.txt"), "1").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "init"]);
        git(&["checkout", "-q", "-b", "lineage/job-1"]);
        std::fs::write(repo.join("a.txt"), "2").unwrap();
        git(&["commit", "-q", "-am", "change a"]);
        std::fs::write(repo.join("b.txt"), "2").unwrap();
        std::fs::write(repo.join("new.txt"), "n").unwrap();
        let mut files = changed_files(repo, Some("main"));
        files.sort();
        assert_eq!(files, vec!["a.txt", "b.txt", "new.txt"]);
    }
}
