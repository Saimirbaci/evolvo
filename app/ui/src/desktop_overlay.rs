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
use crate::types::DesktopSession;

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

    let load_capture = {
        let ctrl = controller.clone();
        move || {
            let ctrl = ctrl.clone();
            spawn_local(async move {
                if let Ok(Some(next)) = interop::take_desktop_capture().await {
                    // A new capture is a new screen: strokes drawn over a
                    // previous frame must not carry over onto this one.
                    ctrl.reset();
                    let app = next
                        .target_app
                        .as_ref()
                        .map(|a| a.name.clone())
                        .unwrap_or_else(|| "unknown".into());
                    route.set(format!("desktop://{app}"));
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

    // Esc closes the overlay, unless the user is typing in a field.
    Effect::new(move |already: Option<()>| {
        if already.is_some() {
            return;
        }
        let Some(win) = web_sys::window() else { return };
        let cb =
            Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |ev: web_sys::KeyboardEvent| {
                if ev.key() != "Escape" || ev.default_prevented() {
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

    view! {
        <div class="desktop-root">
            {move || {
                session.get().map(|s| {
                    let src = format!("data:image/png;base64,{}", s.screenshot_base64);
                    view! { <img class="desktop-frame" src=src alt="" draggable="false" /> }
                })
            }}
            {move || {
                if is_open.get() {
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
        </div>
    }
}
