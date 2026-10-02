mod app;
mod canvas;
mod design_bar;
mod feedback_panel;
mod interop;
mod shell;
mod sketch;
mod toolbar;
mod types;
mod voice;

use app::App;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}
