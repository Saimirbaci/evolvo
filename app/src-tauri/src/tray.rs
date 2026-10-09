//! "Evolvo" menu-bar (system tray) icon — run any app build Evolvo made.
//!
//! Always available, even with the Evolvo window closed:
//! - **Running** — versions Evolvo launched (Stop / Show log),
//! - **Ready to run** — jobs whose build is ready (Run),
//! - **In progress** — jobs an agent is working on, with their phase,
//! - Capture screen (the desktop-overlay shortcut), Open Evolvo, Quit.
//!
//! The title next to the icon shows how many builds are ready to run.
//! Desktop notifications can't carry buttons with the notifier we use, so
//! this menu is where "run it now?" is answered.
//!
//! The menu content is a pure function of the job list ([`build_model`]);
//! a refresh loop rebuilds it whenever a job record changes
//! ([`request_refresh`], called from `lineage_events` and `apprun`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager};

use crate::lineage::LineageEngine;
use crate::state::AppState;
use crate::types::{FeedbackRecord, LineageJobRecord, LineageJobStatus};

pub const TRAY_ID: &str = "evolvo";
const MAX_PER_SECTION: usize = 8;

static DIRTY: AtomicBool = AtomicBool::new(true);

/// Ask the refresh loop to rebuild the menu (cheap; coalesced).
pub fn request_refresh() {
    DIRTY.store(true, Ordering::Relaxed);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayEntry {
    Header(String),
    /// Clickable: `id` is `run:<job>`, `stop:<job>`, `log:<job>`,
    /// `capture`, `open` or `quit`.
    Action {
        id: String,
        label: String,
    },
    /// Greyed-out information line.
    Info(String),
    Separator,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TrayModel {
    pub entries: Vec<TrayEntry>,
    pub ready_count: usize,
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

fn job_label(job: &LineageJobRecord, feedback: Option<&FeedbackRecord>) -> String {
    let app = feedback
        .and_then(|f| f.target_app.as_ref())
        .map(|t| t.name.clone())
        .unwrap_or_else(|| "Evolvo".into());
    let what = if job.title.trim().is_empty() {
        feedback
            .map(|f| f.feedback_text.clone())
            .unwrap_or_default()
    } else {
        job.title.clone()
    };
    format!("{app} — {}", short(&what, 42))
}

/// Human form of an accelerator: `CommandOrControl+Shift+E` → `⌘⇧E`.
pub fn pretty_shortcut(accel: &str) -> String {
    accel
        .split('+')
        .map(|part| match part.trim() {
            "CommandOrControl" | "CmdOrCtrl" | "Command" | "Cmd" | "Super" => "⌘".to_string(),
            "Shift" => "⇧".to_string(),
            "Alt" | "Option" => "⌥".to_string(),
            "Control" | "Ctrl" => "⌃".to_string(),
            other => other.to_uppercase(),
        })
        .collect()
}

/// Menu content for the current jobs, newest first per section.
pub fn build_model(
    jobs: &[(LineageJobRecord, Option<FeedbackRecord>)],
    is_running: impl Fn(&str) -> bool,
    shortcut: &str,
) -> TrayModel {
    let mut sorted: Vec<&(LineageJobRecord, Option<FeedbackRecord>)> = jobs.iter().collect();
    sorted.sort_by_key(|(j, _)| std::cmp::Reverse(j.updated_at_unix_ms));

    let mut running = Vec::new();
    let mut ready = Vec::new();
    let mut progress = Vec::new();
    for (job, fb) in sorted {
        let label = job_label(job, fb.as_ref());
        let resolving = fb
            .as_ref()
            .and_then(|f| f.target_app.as_ref())
            .is_some_and(|t| t.repo_status == "resolving");
        if is_running(&job.id) {
            running.push((job.id.clone(), label));
            continue;
        }
        match job.status {
            LineageJobStatus::BuildReady if job.worktree_path.is_some() => {
                ready.push((job.id.clone(), label))
            }
            LineageJobStatus::Implementing | LineageJobStatus::Triaging => {
                progress.push(format!("{label} · agent working"))
            }
            LineageJobStatus::Merging => progress.push(format!("{label} · merging")),
            LineageJobStatus::Planned
                if job.pause_after_plan && job.plan_approved_at_unix_ms.is_none() =>
            {
                progress.push(format!("{label} · plan needs review"))
            }
            LineageJobStatus::Pending if resolving => {
                progress.push(format!("{label} · finding source"))
            }
            _ => {}
        }
    }

    let mut entries = Vec::new();
    if !running.is_empty() {
        entries.push(TrayEntry::Header("Running".into()));
        for (id, label) in running.iter().take(MAX_PER_SECTION) {
            entries.push(TrayEntry::Action {
                id: format!("stop:{id}"),
                label: format!("■ Stop {label}"),
            });
            entries.push(TrayEntry::Action {
                id: format!("log:{id}"),
                label: "      Show log".into(),
            });
        }
        entries.push(TrayEntry::Separator);
    }
    if !ready.is_empty() {
        entries.push(TrayEntry::Header("Ready to run".into()));
        for (id, label) in ready.iter().take(MAX_PER_SECTION) {
            entries.push(TrayEntry::Action {
                id: format!("run:{id}"),
                label: format!("▶ Run {label}"),
            });
        }
        entries.push(TrayEntry::Separator);
    }
    if !progress.is_empty() {
        entries.push(TrayEntry::Header("In progress".into()));
        for line in progress.iter().take(MAX_PER_SECTION) {
            entries.push(TrayEntry::Info(line.clone()));
        }
        entries.push(TrayEntry::Separator);
    }
    if entries.is_empty() {
        entries.push(TrayEntry::Info(format!(
            "No builds yet — press {} over any app",
            pretty_shortcut(shortcut)
        )));
        entries.push(TrayEntry::Separator);
    }
    entries.push(TrayEntry::Action {
        id: "capture".into(),
        label: format!("Capture screen  {}", pretty_shortcut(shortcut)),
    });
    entries.push(TrayEntry::Action {
        id: "open".into(),
        label: "Open Evolvo".into(),
    });
    entries.push(TrayEntry::Action {
        id: "quit".into(),
        label: "Quit Evolvo".into(),
    });
    TrayModel {
        entries,
        ready_count: ready.len(),
    }
}

fn current_model(app: &AppHandle) -> TrayModel {
    let Some(state) = app.try_state::<AppState>() else {
        return TrayModel::default();
    };
    let store = state.store();
    let jobs: Vec<(LineageJobRecord, Option<FeedbackRecord>)> = store
        .list_lineage_jobs()
        .unwrap_or_default()
        .into_iter()
        .map(|job| {
            let fb = store.load_feedback(&job.feedback_id).ok().flatten();
            (job, fb)
        })
        .collect();
    build_model(
        &jobs,
        crate::apprun::is_running,
        &crate::desktop::shortcut_from_env(),
    )
}

fn to_menu(app: &AppHandle, model: &TrayModel) -> tauri::Result<Menu<tauri::Wry>> {
    let menu = Menu::new(app)?;
    for entry in &model.entries {
        match entry {
            TrayEntry::Header(text) => menu.append(&MenuItem::with_id(
                app,
                format!("h:{text}"),
                text,
                false,
                None::<&str>,
            )?)?,
            TrayEntry::Info(text) => {
                menu.append(&MenuItem::new(app, text, false, None::<&str>)?)?
            }
            TrayEntry::Action { id, label } => menu.append(&MenuItem::with_id(
                app,
                id.as_str(),
                label,
                true,
                None::<&str>,
            )?)?,
            TrayEntry::Separator => menu.append(&PredefinedMenuItem::separator(app)?)?,
        }
    }
    Ok(menu)
}

fn refresh(app: &AppHandle) {
    let model = current_model(app);
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    match to_menu(app, &model) {
        Ok(menu) => {
            let _ = tray.set_menu(Some(menu));
        }
        Err(e) => eprintln!("evolvo: tray menu rebuild failed: {e}"),
    }
    let title = (model.ready_count > 0).then(|| model.ready_count.to_string());
    let _ = tray.set_title(title);
    let tip = match model.ready_count {
        0 => "Evolvo".to_string(),
        1 => "Evolvo — 1 build ready to run".to_string(),
        n => format!("Evolvo — {n} builds ready to run"),
    };
    let _ = tray.set_tooltip(Some(tip));
}

fn handle(app: &AppHandle, id: &str) {
    let store = || app.try_state::<AppState>().map(|s| s.store());
    if let Some(job_id) = id.strip_prefix("run:") {
        if let Some(store) = store() {
            let _ =
                LineageEngine::new(&store).append_note(job_id, "run requested from the menu bar");
            crate::runner::launch_iteration_run(store, job_id.to_string());
        }
    } else if let Some(job_id) = id.strip_prefix("stop:") {
        if crate::apprun::stop(job_id) {
            if let Some(store) = store() {
                let _ = LineageEngine::new(&store)
                    .append_note(job_id, "stopped the launched version from the menu bar");
            }
        }
        request_refresh();
    } else if let Some(job_id) = id.strip_prefix("log:") {
        if let Some(store) = store() {
            let log = crate::runner::iteration_run_log_path(store.layout().root(), job_id);
            let opener = if cfg!(target_os = "macos") {
                "open"
            } else {
                "xdg-open"
            };
            let _ = std::process::Command::new(opener).arg(log).spawn();
        }
    } else {
        match id {
            "capture" => crate::desktop::trigger(app),
            "open" => {
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.unminimize();
                    let _ = win.show();
                    let _ = win.set_focus();
                }
                #[cfg(target_os = "macos")]
                let _ = app.show();
            }
            "quit" => app.exit(0),
            _ => {}
        }
    }
}

/// Create the menu-bar icon and start the refresh loop. Failure is logged
/// and non-fatal — Evolvo works without the tray.
pub fn install(app: &AppHandle) {
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Evolvo")
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| handle(app, event.id().as_ref()));
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    if let Err(e) = builder.build(app) {
        eprintln!("evolvo: could not create the menu-bar icon: {e}");
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || loop {
        if DIRTY.swap(false, Ordering::Relaxed) {
            let handle = app.clone();
            // Menus must be built on the main thread on macOS.
            let _ = app.run_on_main_thread(move || refresh(&handle));
        }
        std::thread::sleep(Duration::from_millis(1500));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TargetApp;

    fn job(id: &str, status: LineageJobStatus, updated: u64) -> LineageJobRecord {
        let mut j: LineageJobRecord = serde_json::from_value(serde_json::json!({
            "id": id, "feedbackId": format!("fb-{id}"), "status": "pending",
            "title": "Add a Jira button", "summary": "s",
            "createdAtUnixMs": 1, "updatedAtUnixMs": updated
        }))
        .unwrap();
        j.status = status;
        j.worktree_path = Some("/wt".into());
        j
    }

    fn zed_feedback(status: &str) -> FeedbackRecord {
        let mut fb: FeedbackRecord = serde_json::from_value(serde_json::json!({
            "id": "fb", "feedbackType": "bug", "status": "new", "pageRoute": "/",
            "feedbackText": "x", "screenshotFilename": null, "voiceFilename": null,
            "voiceTranscript": null, "windowWidth": 1, "windowHeight": 1,
            "createdAtUnixMs": 1, "updatedAtUnixMs": 1, "lineageJobId": null
        }))
        .unwrap();
        fb.target_app = Some(TargetApp {
            name: "Zed".into(),
            repo_status: status.into(),
            ..Default::default()
        });
        fb
    }

    fn ids(m: &TrayModel) -> Vec<String> {
        m.entries
            .iter()
            .filter_map(|e| match e {
                TrayEntry::Action { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn sections_follow_job_state() {
        let jobs = vec![
            (
                job("job-ready", LineageJobStatus::BuildReady, 5),
                Some(zed_feedback("resolved")),
            ),
            (
                job("job-run", LineageJobStatus::BuildReady, 4),
                Some(zed_feedback("resolved")),
            ),
            (job("job-work", LineageJobStatus::Implementing, 3), None),
            (
                job("job-find", LineageJobStatus::Pending, 2),
                Some(zed_feedback("resolving")),
            ),
            (job("job-old", LineageJobStatus::Promoted, 1), None),
        ];
        let m = build_model(&jobs, |id| id == "job-run", "CommandOrControl+Shift+E");
        assert_eq!(m.ready_count, 1);
        assert_eq!(
            ids(&m),
            vec![
                "stop:job-run",
                "log:job-run",
                "run:job-ready",
                "capture",
                "open",
                "quit"
            ]
        );
        let infos: Vec<&TrayEntry> = m
            .entries
            .iter()
            .filter(|e| matches!(e, TrayEntry::Info(_)))
            .collect();
        assert_eq!(infos.len(), 2);
        assert!(matches!(&infos[0], TrayEntry::Info(t) if t.contains("agent working")));
        assert!(
            matches!(&infos[1], TrayEntry::Info(t) if t.starts_with("Zed — ") && t.contains("finding source"))
        );
        assert!(m.entries.iter().any(|e| matches!(e, TrayEntry::Action { label, .. } if label == "▶ Run Zed — Add a Jira button")));
    }

    #[test]
    fn empty_state_explains_the_shortcut() {
        let m = build_model(&[], |_| false, "CommandOrControl+Shift+E");
        assert_eq!(m.ready_count, 0);
        assert!(matches!(&m.entries[0], TrayEntry::Info(t) if t.contains("⌘⇧E")));
        assert_eq!(ids(&m), vec!["capture", "open", "quit"]);
    }

    #[test]
    fn pretty_shortcut_uses_mac_symbols() {
        assert_eq!(pretty_shortcut("CommandOrControl+Shift+E"), "⌘⇧E");
        assert_eq!(pretty_shortcut("Ctrl+Alt+w"), "⌃⌥W");
    }
}
