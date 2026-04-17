#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use noide_desktop::commands;
use noide_desktop::state::AppState;

fn main() {
    tauri::Builder::default()
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::app_health,
            commands::submit_feedback,
            commands::list_feedback,
            commands::load_feedback,
            commands::delete_feedback,
            commands::list_sandbox_jobs,
            commands::load_sandbox_job,
            commands::approve_sandbox_job,
            commands::reject_sandbox_job,
            commands::retry_sandbox_job,
            commands::run_sandbox_job,
            commands::append_sandbox_note,
            commands::open_workspace_path,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run NoIDE desktop app");
}
