//! Desktop overlay root — "evolve any app".
//!
//! Mounted instead of `App` when the host opens the borderless
//! `desktop-overlay` window (global shortcut, see `desktop.rs` on the host).
//! The host has already frozen the monitor and identified the frontmost
//! app; this view shows that frame full-bleed with the same Canvas surface,
//! Toolbar and Feedback panel the in-app overlay uses. Submitting goes
//! through the normal `submit_feedback` path with `targetApp` attached.
//!
//! This is a separate surface for *other* applications. The in-app white
//! canvas (Shell + its single Feedback FAB) is untouched.

use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

use crate::canvas::{CanvasController, CanvasSurface};
use crate::feedback_panel::{FeedbackPanel, FeedbackRevisionRequest};
use crate::interop;
use crate::toolbar::Toolbar;
use crate::types::{DesktopSession, PickableWindow};

/// What the overlay is showing: the whole frozen monitor, the window
/// picker, or a single captured window.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum CaptureMode {
    Screen,
    Picking,
    Window,
}

/// Space kept free around a single-window frame: the toolbar runs along
/// the top and the feedback panel (320px + margins) sits on the right.
const WINDOW_FRAME_MARGIN: f64 = 24.0;
const WINDOW_FRAME_TOP: f64 = 72.0;
const WINDOW_FRAME_RIGHT: f64 = 364.0;

/// Fit an image of `img_w`×`img_h` CSS px into the given box, centred,
/// never upscaling. Returns `(x, y, w, h)`.
pub(crate) fn fit_rect(
    box_x: f64,
    box_y: f64,
    box_w: f64,
    box_h: f64,
    img_w: f64,
    img_h: f64,
) -> (f64, f64, f64, f64) {
    if img_w <= 0.0 || img_h <= 0.0 || box_w <= 0.0 || box_h <= 0.0 {
        return (box_x, box_y, box_w.max(0.0), box_h.max(0.0));
    }
    let scale = (box_w / img_w).min(box_h / img_h).min(1.0);
    let (w, h) = (img_w * scale, img_h * scale);
    (box_x + (box_w - w) / 2.0, box_y + (box_h - h) / 2.0, w, h)
}

/// Front-most window whose rect contains the point (rects are front to back).
pub(crate) fn window_at(windows: &[PickableWindow], x: f64, y: f64) -> Option<usize> {
    windows.iter().position(|w| {
        x >= w.x as f64
            && y >= w.y as f64
            && x < (w.x as f64 + w.width as f64)
            && y < (w.y as f64 + w.height as f64)
    })
}

fn viewport() -> (f64, f64) {
    let Some(win) = web_sys::window() else {
        return (0.0, 0.0);
    };
    let w = win
        .inner_width()
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let h = win
        .inner_height()
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    (w, h)
}

fn device_pixel_ratio() -> f64 {
    web_sys::window()
        .map(|w| w.device_pixel_ratio())
        .filter(|r| *r > 0.0)
        .unwrap_or(1.0)
}

fn route_for(session: &DesktopSession) -> String {
    let app = session
        .target_app
        .as_ref()
        .map(|a| a.name.clone())
        .unwrap_or_else(|| "unknown".into());
    format!("desktop://{app}")
}

/// Provided to the overlay's `FeedbackPanel` so it submits the frozen frame
/// and target app instead of capturing Evolvo's own window.
#[derive(Copy, Clone)]
pub struct DesktopCaptureContext {
    pub session: RwSignal<Option<DesktopSession>>,
}

/// True when the host injected `window.__EVOLVO_MODE__ = 'desktop'`.
pub fn is_desktop_mode() -> bool {
    web_sys::window()
        .and_then(|w| js_sys::Reflect::get(&w, &"__EVOLVO_MODE__".into()).ok())
        .and_then(|v| v.as_string())
        .map(|m| m == "desktop")
        .unwrap_or(false)
}

#[component]
pub fn DesktopOverlayApp() -> impl IntoView {
    let session: RwSignal<Option<DesktopSession>> = RwSignal::new(None);
    let is_open = RwSignal::new(false);
    let route = RwSignal::new(String::from("desktop://"));
    let revision_request: RwSignal<Option<FeedbackRevisionRequest>> = RwSignal::new(None);
    let controller = CanvasController::new();
    // Bumped per capture to remount the FeedbackPanel: each capture starts
    // from a clean draft (text, voice, revision ancestry) instead of
    // inheriting whatever an abandoned earlier capture left behind.
    let generation = RwSignal::new(0u32);
    provide_context(DesktopCaptureContext { session });
    // The whole-screen capture from the shortcut; window mode swaps the
    // displayed/submitted frame in `session`, and S swaps this back.
    let screen_session: RwSignal<Option<DesktopSession>> = RwSignal::new(None);
    let mode = RwSignal::new(CaptureMode::Screen);
    let hovered: RwSignal<Option<usize>> = RwSignal::new(None);
    let picker_status: RwSignal<Option<String>> = RwSignal::new(None);
    let picking_busy = RwSignal::new(false);

    let load_capture = {
        let ctrl = controller.clone();
        move || {
            let ctrl = ctrl.clone();
            spawn_local(async move {
                if let Ok(Some(next)) = interop::take_desktop_capture().await {
                    // A new capture is a new screen: strokes drawn over a
                    // previous frame must not carry over onto this one.
                    ctrl.reset();
                    route.set(route_for(&next));
                    mode.set(CaptureMode::Screen);
                    hovered.set(None);
                    picker_status.set(None);
                    screen_session.set(Some(next.clone()));
                    let revision =
                        next.revision_of
                            .clone()
                            .map(|feedback_id| FeedbackRevisionRequest {
                                feedback_id,
                                agent: next.revision_agent.unwrap_or_default(),
                            });
                    session.set(Some(next));
                    generation.update(|g| *g = g.wrapping_add(1));
                    // "Revise on Canvas": the fresh panel hydrates the earlier
                    // record's annotations over the original frozen frame.
                    revision_request.set(revision);
                    is_open.set(true);
                }
            });
        }
    };

    // Initial capture (first shortcut press creates this window) plus every
    // later press, which re-shows the hidden window and emits the event.
    {
        let load = load_capture.clone();
        Effect::new(move |already: Option<()>| {
            if already.is_some() {
                return;
            }
            load();
            let load = load.clone();
            spawn_local(async move {
                if let Ok(listener) = interop::listen_desktop_capture(move |_| load()).await {
                    // The overlay window lives as long as the process.
                    std::mem::forget(listener);
                }
            });
        });
    }

    // Closing (× on the panel, Esc, or a successful submit — all of which
    // drop `is_open`) hides the window and returns focus to the target app.
    {
        let ctrl = controller.clone();
        Effect::new(move |prev: Option<bool>| {
            let open = is_open.get();
            if prev == Some(true) && !open {
                ctrl.reset();
                session.set(None);
                spawn_local(async move {
                    let _ = interop::close_desktop_overlay().await;
                });
            }
            open
        });
    }

    // W: pick a window to draw on. Starts on the active window so Enter
    // alone means "the window I was just using".
    let enter_picker = {
        let ctrl = controller.clone();
        move || {
            let Some(screen) = screen_session.get_untracked() else {
                return;
            };
            if screen.revision_of.is_some() {
                // A revision redraws over its original frame; no re-capture.
                return;
            }
            if screen.windows.is_empty() {
                picker_status.set(Some("No other windows found on this screen.".into()));
                return;
            }
            if mode.get_untracked() == CaptureMode::Window {
                ctrl.reset();
                route.set(route_for(&screen));
                session.set(Some(screen.clone()));
            }
            let start = screen.windows.iter().position(|w| w.focused).unwrap_or(0);
            hovered.set(Some(start));
            picker_status.set(None);
            mode.set(CaptureMode::Picking);
        }
    };

    let pick_window = {
        let ctrl = controller.clone();
        move |idx: usize| {
            if picking_busy.get_untracked() {
                return;
            }
            let Some(screen) = screen_session.get_untracked() else {
                return;
            };
            let Some(win) = screen.windows.get(idx).cloned() else {
                return;
            };
            picking_busy.set(true);
            picker_status.set(Some(format!("Capturing {}…", win.app_name)));
            let ctrl = ctrl.clone();
            spawn_local(async move {
                match interop::capture_desktop_window(win.id).await {
                    Ok(capture) => {
                        // Strokes drawn over the whole screen don't line up
                        // with a single window, so the drawing starts fresh.
                        ctrl.reset();
                        let mut next = screen.clone();
                        next.screenshot_base64 = capture.screenshot_base64;
                        next.width = capture.width;
                        next.height = capture.height;
                        if capture.target_app.is_some() {
                            next.target_app = capture.target_app;
                        }
                        route.set(route_for(&next));
                        session.set(Some(next));
                        picker_status.set(None);
                        mode.set(CaptureMode::Window);
                    }
                    Err(e) => {
                        picker_status.set(Some(format!("Could not capture that window: {e}")))
                    }
                }
                picking_busy.set(false);
            });
        }
    };

    // S: back to the whole screen.
    let use_screen = {
        let ctrl = controller.clone();
        move || {
            let Some(screen) = screen_session.get_untracked() else {
                return;
            };
            if mode.get_untracked() == CaptureMode::Window {
                ctrl.reset();
            }
            route.set(route_for(&screen));
            session.set(Some(screen));
            picker_status.set(None);
            mode.set(CaptureMode::Screen);
        }
    };

    // Overlay keys (ignored while typing in a field):
    //   W — pick a window · S — whole screen · Enter — take the highlighted
    //   window while picking · Esc — leave the picker, otherwise close.
    {
        let enter_picker = enter_picker.clone();
        let pick_window = pick_window.clone();
        let use_screen = use_screen.clone();
        Effect::new(move |already: Option<()>| {
            if already.is_some() {
                return;
            }
            let Some(win) = web_sys::window() else { return };
            let enter_picker = enter_picker.clone();
            let pick_window = pick_window.clone();
            let use_screen = use_screen.clone();
            let cb = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(
                move |ev: web_sys::KeyboardEvent| {
                    if !is_open.get_untracked() || ev.default_prevented() {
                        return;
                    }
                    if ev.meta_key() || ev.ctrl_key() || ev.alt_key() {
                        return;
                    }
                    let typing = ev
                        .target()
                        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                        .map(|el| {
                            let tag = el.tag_name().to_ascii_lowercase();
                            tag == "input"
                                || tag == "textarea"
                                || el.has_attribute("contenteditable")
                        })
                        .unwrap_or(false);
                    if typing {
                        return;
                    }
                    let picking = mode.get_untracked() == CaptureMode::Picking;
                    match ev.key().as_str() {
                        "w" | "W" => {
                            enter_picker();
                            ev.prevent_default();
                        }
                        "s" | "S" if mode.get_untracked() != CaptureMode::Screen => {
                            use_screen();
                            ev.prevent_default();
                        }
                        "Enter" if picking => {
                            if let Some(idx) = hovered.get_untracked() {
                                pick_window(idx);
                            }
                            ev.prevent_default();
                        }
                        "Escape" if picking => {
                            use_screen();
                            ev.prevent_default();
                        }
                        _ => {}
                    }
                },
            );
            let _ = win.add_event_listener_with_callback("keydown", cb.as_ref().unchecked_ref());
            cb.forget();
        });
    }

    // Esc closes the overlay, unless the user is typing in a field.
    Effect::new(move |already: Option<()>| {
        if already.is_some() {
            return;
        }
        let Some(win) = web_sys::window() else { return };
        let cb =
            Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |ev: web_sys::KeyboardEvent| {
                // While picking, Esc only leaves the picker (handled above).
                if ev.key() != "Escape"
                    || ev.default_prevented()
                    || mode.get_untracked() == CaptureMode::Picking
                {
                    return;
                }
                let typing = ev
                    .target()
                    .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                    .map(|el| {
                        let tag = el.tag_name().to_ascii_lowercase();
                        tag == "input" || tag == "textarea" || el.has_attribute("contenteditable")
                    })
                    .unwrap_or(false);
                if !typing {
                    is_open.set(false);
                }
            });
        let _ = win.add_event_listener_with_callback("keydown", cb.as_ref().unchecked_ref());
        cb.forget();
    });

    let ctrl_toolbar = controller.clone();
    let ctrl_canvas = controller.clone();
    let ctrl_panel = controller.clone();
    let pick_click = pick_window.clone();
    let mode_window = enter_picker.clone();
    let mode_screen = use_screen.clone();

    view! {
        <div
            class="desktop-root"
            class:desktop-mode-window=move || mode.get() == CaptureMode::Window
        >
            {move || {
                session.get().map(|s| {
                    let src = format!("data:image/png;base64,{}", s.screenshot_base64);
                    if mode.get() == CaptureMode::Window {
                        // One window: centred in the space left by the
                        // toolbar and the panel, at most 1:1 in points.
                        let (vw, vh) = viewport();
                        let dpr = device_pixel_ratio();
                        let (x, y, w, h) = fit_rect(
                            WINDOW_FRAME_MARGIN,
                            WINDOW_FRAME_TOP,
                            vw - WINDOW_FRAME_MARGIN - WINDOW_FRAME_RIGHT,
                            vh - WINDOW_FRAME_TOP - WINDOW_FRAME_MARGIN,
                            s.width as f64 / dpr,
                            s.height as f64 / dpr,
                        );
                        view! {
                            <img
                                class="desktop-frame desktop-frame-window"
                                src=src
                                alt=""
                                draggable="false"
                                style:left=format!("{x}px")
                                style:top=format!("{y}px")
                                style:width=format!("{w}px")
                                style:height=format!("{h}px")
                            />
                        }
                        .into_any()
                    } else {
                        view! { <img class="desktop-frame" src=src alt="" draggable="false" /> }
                            .into_any()
                    }
                })
            }}
            {move || {
                if is_open.get() && mode.get() != CaptureMode::Picking {
                    view! {
                        <div class="canvas-overlay">
                            <Toolbar controller=ctrl_toolbar.clone() />
                            <CanvasSurface controller=ctrl_canvas.clone() />
                        </div>
                    }
                    .into_any()
                } else {
                    view! { <span></span> }.into_any()
                }
            }}
            {move || {
                let _ = generation.get();
                view! {
                    <FeedbackPanel
                        controller=ctrl_panel.clone()
                        route=route
                        is_open=is_open
                        revision_request=revision_request
                    />
                }
            }}
            {move || {
                if mode.get() != CaptureMode::Picking {
                    return view! { <span></span> }.into_any();
                }
                let windows = screen_session
                    .get()
                    .map(|s| s.windows)
                    .unwrap_or_default();
                let hit_windows = windows.clone();
                let pick = pick_click.clone();
                view! {
                    <div
                        class="desktop-picker"
                        role="listbox"
                        aria-label="Pick a window to draw on"
                        on:mousemove=move |ev: web_sys::MouseEvent| {
                            let idx = window_at(&hit_windows, ev.client_x() as f64, ev.client_y() as f64);
                            if idx.is_some() && idx != hovered.get_untracked() {
                                hovered.set(idx);
                            }
                        }
                        on:click=move |_| {
                            if let Some(idx) = hovered.get_untracked() {
                                pick(idx);
                            }
                        }
                    >
                        {move || {
                            hovered.get().and_then(|i| windows.get(i).cloned()).map(|w| {
                                let label = if w.title.trim().is_empty() {
                                    w.app_name.clone()
                                } else {
                                    format!("{} — {}", w.app_name, w.title)
                                };
                                view! {
                                    <div
                                        class="desktop-picker-highlight"
                                        style:left=format!("{}px", w.x)
                                        style:top=format!("{}px", w.y)
                                        style:width=format!("{}px", w.width)
                                        style:height=format!("{}px", w.height)
                                    >
                                        <span class="desktop-picker-label">{label}</span>
                                    </div>
                                }
                            })
                        }}
                    </div>
                }
                .into_any()
            }}
            {move || {
                // Mode switch: only for fresh captures (a revision redraws
                // over its saved frame).
                let fresh = session.get().is_some_and(|s| s.revision_of.is_none());
                if !is_open.get() || !fresh {
                    return view! { <span></span> }.into_any();
                }
                let to_screen = mode_screen.clone();
                let to_window = mode_window.clone();
                let hint = move || match (picker_status.get(), mode.get()) {
                    (Some(msg), _) => msg,
                    (None, CaptureMode::Picking) => {
                        "Click a window · Enter: active window · Esc: back".to_string()
                    }
                    (None, _) => "Switching clears your drawing".to_string(),
                };
                view! {
                    <div class="desktop-modebar" role="toolbar" aria-label="Capture area">
                        <button
                            class="desktop-mode-btn"
                            class:active=move || mode.get() == CaptureMode::Screen
                            title="Draw on the whole screen (S)"
                            aria-label="Draw on the whole screen"
                            on:click=move |_| to_screen()
                        >
                            "Screen "<kbd>"S"</kbd>
                        </button>
                        <button
                            class="desktop-mode-btn"
                            class:active=move || mode.get() != CaptureMode::Screen
                            title="Pick one window to draw on (W)"
                            aria-label="Pick one window to draw on"
                            on:click=move |_| to_window()
                        >
                            "Window "<kbd>"W"</kbd>
                        </button>
                        <span class="desktop-mode-hint">{hint}</span>
                    </div>
                }
                .into_any()
            }}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(id: u32, x: i32, y: i32, w: u32, h: u32) -> PickableWindow {
        PickableWindow {
            id,
            app_name: format!("app{id}"),
            title: String::new(),
            x,
            y,
            width: w,
            height: h,
            focused: false,
        }
    }

    #[test]
    fn fit_rect_centres_without_upscaling() {
        // Small image: kept at 1:1, centred.
        assert_eq!(
            fit_rect(0.0, 0.0, 1000.0, 800.0, 400.0, 200.0),
            (300.0, 300.0, 400.0, 200.0)
        );
        // Wide image: scaled to the box width.
        assert_eq!(
            fit_rect(10.0, 20.0, 500.0, 500.0, 1000.0, 500.0),
            (10.0, 145.0, 500.0, 250.0)
        );
        // Degenerate input falls back to the box.
        assert_eq!(
            fit_rect(1.0, 2.0, 3.0, 4.0, 0.0, 10.0),
            (1.0, 2.0, 3.0, 4.0)
        );
    }

    #[test]
    fn window_at_prefers_frontmost() {
        let ws = vec![win(1, 100, 100, 200, 200), win(2, 0, 0, 1000, 800)];
        assert_eq!(window_at(&ws, 150.0, 150.0), Some(0));
        assert_eq!(window_at(&ws, 50.0, 50.0), Some(1));
        assert_eq!(window_at(&ws, 1200.0, 50.0), None);
        // Right/bottom edges are exclusive.
        assert_eq!(window_at(&ws, 300.0, 150.0), Some(1));
    }
}
