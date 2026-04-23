mod app;
mod canvas;
mod feedback_panel;
mod interop;
mod shell;
mod toolbar;
mod types;
mod voice;

use app::App;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}
