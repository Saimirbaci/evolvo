#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use evolvo_desktop::commands;
use evolvo_desktop::state::AppState;

fn main() {
    // Headless `validate` subcommand: lets an in-worktree agent session run
    // the exact stage gate the pipeline will apply (see validate_cli.rs).
    // Handled before the Tauri builder so no window/IPC is ever set up.
    let argv: Vec<String> = std::env::args().collect();
    if argv.get(1).map(String::as_str) == Some("validate") {
        std::process::exit(evolvo_desktop::validate_cli::main(&argv[2..]));
    }

    let app_state = AppState::new();
    let watcher_root = app_state.store().layout().root().to_path_buf();
    // Crash recovery: jobs whose runner thread died with the previous
    // process must not sit in `Implementing` forever.
    let orphaned = evolvo_desktop::runner::reconcile_orphaned_jobs(&app_state.store());
    if orphaned > 0 {
        eprintln!("evolvo: marked {orphaned} orphaned lineage job(s) failed after restart");
    }

    // Workspace retention: reclaim disk from old Failed/Rejected lineage
    // workspaces. Runs on a background thread (it walks + may `git
    // worktree prune` every lineage workspace, which is not something
    // startup should block on) after reconciliation, so it only ever
    // considers jobs whose terminal status is already settled.
    {
        let store = app_state.store();
        std::thread::spawn(move || {
            let max_age_days = evolvo_desktop::runner::workspace_retention_days();
            let summary = evolvo_desktop::runner::cleanup_old_workspaces(&store, max_age_days);
            if summary.removed > 0 || summary.quarantined > 0 {
                eprintln!(
                    "evolvo: workspace retention — removed {} old workspace(s), quarantined {} orphaned workspace(s), kept {}",
                    summary.removed, summary.quarantined, summary.kept
                );
            }
        });
    }

    let desktop_shortcut = evolvo_desktop::desktop::shortcut_from_env();

    tauri::Builder::default()
        .manage(app_state)
        .manage(evolvo_desktop::desktop::DesktopState::default())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state() == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                        evolvo_desktop::desktop::trigger(app);
                    }
                })
                .build(),
        )
        .setup(move |app| {
            // "Evolvo" menu-bar icon: run/stop builds, capture, open, quit.
            evolvo_desktop::tray::install(app.handle());
            // Desktop overlay ("evolve any app"): a failure to grab the
            // shortcut (taken by another app, bad accelerator in
            // EVOLVO_DESKTOP_SHORTCUT) must not stop Evolvo opening.
            {
                use tauri_plugin_global_shortcut::GlobalShortcutExt;
                if let Err(err) = app.global_shortcut().register(desktop_shortcut.as_str()) {
                    eprintln!(
                        "evolvo: could not register desktop overlay shortcut {desktop_shortcut}: {err}"
                    );
                }
            }
            evolvo_desktop::lineage_events::install_tauri_sink(
                app.handle().clone(),
                watcher_root.clone(),
            );
            if let Err(err) =
                evolvo_desktop::lineage_events::start_workspace_watcher(watcher_root.clone())
            {
                // The 60-second UI reconciliation remains a recovery path;
                // watcher startup failure must not stop Evolvo opening.
                eprintln!("evolvo: {err}");
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_health,
            commands::submit_feedback,
            commands::list_feedback,
            commands::load_feedback,
            commands::load_feedback_for_revision,
            commands::delete_feedback,
            commands::list_lineage_jobs,
            commands::load_lineage_job,
            commands::approve_lineage_job,
            commands::answer_plan_questions,
            commands::reject_lineage_job,
            commands::retry_lineage_job,
            commands::run_lineage_job,
            commands::append_lineage_note,
            commands::open_workspace_path,
            commands::capture_window_png,
            commands::take_desktop_capture,
            commands::close_desktop_overlay,
            commands::read_job_activity,
            commands::stop_lineage_run,
            commands::delete_lineage_job,
            commands::capture_desktop_window,
            commands::open_desktop_revision,
            commands::resolve_target_repo,
            commands::open_external_url,
            commands::list_job_stages,
            commands::read_job_plan,
            commands::tail_stage_log,
            commands::resume_lineage_job,
            commands::list_available_agents,
            commands::list_local_delegate_availability,
            commands::read_delegation_summary,
            commands::preview_lineage_evolution,
            commands::export_lineage,
            commands::import_lineage_bundle,
            commands::save_design,
            commands::save_design_draft,
            commands::list_designs,
            commands::load_design,
            commands::load_design_draft,
            commands::delete_design,
            commands::clear_design_draft,
            commands::generate_sketch,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Evolvo desktop app");
}
