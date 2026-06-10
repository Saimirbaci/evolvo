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

    tauri::Builder::default()
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::app_health,
            commands::submit_feedback,
            commands::list_feedback,
            commands::load_feedback,
            commands::delete_feedback,
            commands::list_lineage_jobs,
            commands::load_lineage_job,
            commands::approve_lineage_job,
            commands::reject_lineage_job,
            commands::retry_lineage_job,
            commands::run_lineage_job,
            commands::append_lineage_note,
            commands::open_workspace_path,
            commands::capture_window_png,
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
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Evolvo desktop app");
}
