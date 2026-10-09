//! Desktop notifications for lineage jobs.
//!
//! The user is usually in another app (that's the point of the desktop
//! overlay), so job milestones are pushed as OS notifications: agent
//! started, plan ready for review, ready for review, failed, merged — plus
//! source-repo resolution trouble from `external.rs`.
//!
//! Every job save is published through `lineage_events`; `observe_job`
//! remembers the last status per job and notifies only on a *change*, so
//! the same status seen via the store and via the file watcher notifies
//! once, and nothing fires for jobs merely loaded at startup.
//! `EVOLVO_NOTIFICATIONS=0` turns this off.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::types::{LineageJobRecord, LineageJobStatus};

fn enabled() -> bool {
    !cfg!(test)
        && std::env::var("EVOLVO_NOTIFICATIONS")
            .map(|v| v.trim() != "0")
            .unwrap_or(true)
}

/// Fire-and-forget OS notification. Never blocks the caller and never
/// fails loudly — a missing notifier must not affect the pipeline.
pub fn desktop_notify(title: &str, body: &str) {
    if !enabled() {
        return;
    }
    let (title, body) = (title.to_string(), body.to_string());
    std::thread::spawn(move || send(&title, &body));
}

#[cfg(target_os = "macos")]
fn send(title: &str, body: &str) {
    let script = format!(
        "display notification \"{}\" with title \"{}\"",
        applescript_escape(body),
        applescript_escape(title)
    );
    let _ = std::process::Command::new("osascript")
        .args(["-e", &script])
        .output();
}

#[cfg(target_os = "linux")]
fn send(title: &str, body: &str) {
    let _ = std::process::Command::new("notify-send")
        .args([title, body])
        .output();
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn send(_title: &str, _body: &str) {}

/// Escape for an AppleScript string literal; newlines become spaces.
pub fn applescript_escape(s: &str) -> String {
    s.chars()
        .take(240)
        .map(|c| match c {
            '\n' | '\r' => ' ',
            c => c,
        })
        .collect::<String>()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

fn last_status() -> &'static Mutex<HashMap<String, LineageJobStatus>> {
    static MAP: OnceLock<Mutex<HashMap<String, LineageJobStatus>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The notification for a status transition, if it's one worth an
/// interruption. `None` for the first observation of a job.
pub fn transition_message(
    previous: Option<LineageJobStatus>,
    job: &LineageJobRecord,
) -> Option<(String, String)> {
    let previous = previous?;
    if previous == job.status {
        return None;
    }
    let title = format!("Evolvo — {}", short(&job.title, 60));
    let body = match job.status {
        LineageJobStatus::Implementing
            if previous != LineageJobStatus::Planned || job.plan_approved_at_unix_ms.is_some() =>
        {
            format!("{} started working", job.agent.label())
        }
        LineageJobStatus::Planned
            if job.pause_after_plan && job.plan_approved_at_unix_ms.is_none() =>
        {
            "The plan is ready for your review".to_string()
        }
        LineageJobStatus::BuildReady if previous == LineageJobStatus::Merging => {
            // Promotion bounced back (precondition, conflict or gate).
            format!("Merge didn't go through — {}", last_note(job))
        }
        LineageJobStatus::BuildReady => {
            "Ready for review — run it from the Evolvo menu bar".to_string()
        }
        LineageJobStatus::Failed => format!("Failed — {}", last_note(job)),
        LineageJobStatus::Promoted => "Merged into the source repo".to_string(),
        _ => return None,
    };
    Some((title, body))
}

fn last_note(job: &LineageJobRecord) -> String {
    job.notes
        .last()
        .map(|n| short(n, 140))
        .unwrap_or_else(|| "see the Lineage page".to_string())
}

fn short(s: &str, max: usize) -> String {
    let line = s.lines().next().unwrap_or("").trim();
    if line.chars().count() <= max {
        line.to_string()
    } else {
        let mut out: String = line.chars().take(max - 1).collect();
        out.push('…');
        out
    }
}

/// Called for every published job save (host workspace only).
pub fn observe_job(job: &LineageJobRecord) {
    let previous = {
        let Ok(mut map) = last_status().lock() else {
            return;
        };
        map.insert(job.id.clone(), job.status)
    };
    if let Some((title, body)) = transition_message(previous, job) {
        desktop_notify(&title, &body);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(status: LineageJobStatus) -> LineageJobRecord {
        let mut j: LineageJobRecord = serde_json::from_value(serde_json::json!({
            "id": "job-1", "feedbackId": "fb-1", "status": "pending",
            "title": "Add a Jira button to Zed", "summary": "s",
            "createdAtUnixMs": 1, "updatedAtUnixMs": 1
        }))
        .unwrap();
        j.status = status;
        j
    }

    #[test]
    fn notifies_only_on_meaningful_transitions() {
        let s = LineageJobStatus::Implementing;
        assert!(
            transition_message(None, &job(s)).is_none(),
            "first sighting is silent"
        );
        assert!(
            transition_message(Some(s), &job(s)).is_none(),
            "no change, no ping"
        );
        let (title, body) = transition_message(Some(LineageJobStatus::Pending), &job(s)).unwrap();
        assert!(title.contains("Add a Jira button"));
        assert!(body.contains("started"));
        let (_, body) = transition_message(Some(s), &job(LineageJobStatus::BuildReady)).unwrap();
        assert!(body.starts_with("Ready for review"));
        let mut failed = job(LineageJobStatus::Failed);
        failed.notes = vec!["build gates rejected the worktree".into()];
        let (_, body) = transition_message(Some(s), &failed).unwrap();
        assert!(body.contains("build gates rejected"));
        let (_, body) = transition_message(
            Some(LineageJobStatus::Merging),
            &job(LineageJobStatus::BuildReady),
        )
        .unwrap();
        assert!(body.starts_with("Merge didn't go through"));
        assert!(transition_message(Some(s), &job(LineageJobStatus::Merging)).is_none());
    }

    #[test]
    fn applescript_escape_neutralises_quotes_and_newlines() {
        assert_eq!(applescript_escape("a \"b\"\\c\nd"), "a \\\"b\\\"\\\\c d");
    }
}
