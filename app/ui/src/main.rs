mod app;
mod canvas;
mod design_bar;
mod desktop_overlay;
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
    // The host's desktop overlay window (global shortcut) mounts only the
    // canvas + feedback panel over a frozen screen; every other window gets
    // the normal Shell-wrapped app.
    if desktop_overlay::is_desktop_mode() {
        leptos::mount::mount_to_body(desktop_overlay::DesktopOverlayApp);
    } else {
        leptos::mount::mount_to_body(App);
    }
}
