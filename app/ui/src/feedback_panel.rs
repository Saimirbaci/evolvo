use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    window, CanvasRenderingContext2d, Element, HtmlCanvasElement, HtmlElement, HtmlImageElement,
    HtmlTextAreaElement, PointerEvent,
};

use crate::canvas::CanvasController;
use crate::interop;
use crate::types::{FeedbackType, SubmitFeedbackPayload};
use crate::voice::{VoiceRecorder, VoiceState};

#[component]
pub fn FeedbackPanel(
    controller: CanvasController,
    route: RwSignal<String>,
    is_open: RwSignal<bool>,
) -> impl IntoView {
    let feedback_type = RwSignal::new(FeedbackType::Bug);
    let text = RwSignal::new(String::new());
    let submitting = RwSignal::new(false);
    let status: RwSignal<Option<Result<String, String>>> = RwSignal::new(None);
    let voice = VoiceState::new();

    let submit = {
        let ctrl = controller.clone();
        let voice = voice.clone();
        move |_| {
            let ready = !text.get_untracked().trim().is_empty()
                || !ctrl.shapes.get_untracked().is_empty()
                || !ctrl.pasted_base64.get_untracked().is_empty();
            if !ready || submitting.get_untracked() {
                return;
            }
            submitting.set(true);
            status.set(None);
            let anno_url = ctrl.to_png_data_url();
            let annotations = ctrl.annotations_json();
            let pasted = ctrl.pasted_base64.get_untracked();
            let (win_w, win_h) = window_size();
            let transcript = {
                let t = voice.transcript.get_untracked();
                if t.trim().is_empty() {
                    None
                } else {
                    Some(t)
                }
            };
            let feedback_type_v = feedback_type.get_untracked();
            let route_v = route.get_untracked();
            let text_v = text.get_untracked();
            let voice_b64 = voice.base64.get_untracked();
            let voice_mime = voice.mime_type.get_untracked();

            let ctrl_reset = ctrl.clone();
            let voice_reset = voice.clone();
            spawn_local(async move {
                // Hide overlays so the screenshot captures the underlying page
                // without the canvas surface or this feedback panel obscuring it.
                let hidden = hide_overlays_for_capture();
                wait_two_frames().await;
                let page_b64 = interop::capture_window_png().await.ok();
                restore_overlays(&hidden);

                // Composite: page as backdrop, annotation PNG stretched on top.
                let screenshot = match (page_b64.as_deref(), anno_url.as_deref()) {
                    (Some(page), Some(anno)) => composite_page_and_annotations(page, anno)
                        .await
                        .or_else(|| Some(page.to_string())),
                    (Some(page), None) => Some(page.to_string()),
                    (None, Some(anno)) => Some(
                        anno.split(',')
                            .nth(1)
                            .map(str::to_string)
                            .unwrap_or_default(),
                    ),
                    (None, None) => None,
                }
                .filter(|s| !s.is_empty());

                let payload = SubmitFeedbackPayload {
                    feedback_type: feedback_type_v,
                    page_route: route_v,
                    feedback_text: text_v,
                    annotations,
                    pasted_images_base64: pasted,
                    screenshot_base64: screenshot,
                    voice_base64: voice_b64,
                    voice_mime_type: voice_mime,
                    voice_transcript: transcript,
                    window_width: win_w,
                    window_height: win_h,
                };

                match interop::submit_feedback(&payload).await {
                    Ok(record) => {
                        status.set(Some(Ok(format!("Sent • queued as {}", record.id))));
                        ctrl_reset.clear_all();
                        voice_reset.clear();
                        text.set(String::new());
                        is_open.set(false);
                    }
                    Err(err) => {
                        status.set(Some(Err(err)));
                    }
                }
                submitting.set(false);
            });
        }
    };

    let disabled_ctrl = controller.clone();
    let thumbs_ctrl = controller.clone();
    let voice_view = voice.clone();

    let offset: RwSignal<(f64, f64)> = RwSignal::new((0.0, 0.0));
    let drag_origin: RwSignal<Option<(f64, f64, f64, f64)>> = RwSignal::new(None);
    // Explicit size overrides. `None` means "use the CSS default" (width 320px,
    // height stretched between top:20 and bottom:20). Once the user drags the
    // resize handle we switch to fixed pixel dimensions.
    let size: RwSignal<Option<(f64, f64)>> = RwSignal::new(None);
    let resize_origin: RwSignal<Option<(f64, f64, f64, f64)>> = RwSignal::new(None);

    let on_resize_down = move |ev: PointerEvent| {
        ev.prevent_default();
        ev.stop_propagation();
        if let Some(target) = ev
            .current_target()
            .and_then(|t| t.dyn_into::<Element>().ok())
        {
            let _ = target.set_pointer_capture(ev.pointer_id());
        }
        let (w, h) = size.get_untracked().unwrap_or_else(current_panel_size);
        resize_origin.set(Some((ev.client_x() as f64, ev.client_y() as f64, w, h)));
    };
    let on_resize_move = move |ev: PointerEvent| {
        if let Some((sx, sy, w0, h0)) = resize_origin.get_untracked() {
            // Panel is anchored top/right, so dragging left grows width and
            // dragging down grows height.
            let dx = sx - ev.client_x() as f64;
            let dy = ev.client_y() as f64 - sy;
            let w = (w0 + dx).clamp(260.0, 900.0);
            let h = (h0 + dy).clamp(240.0, 1400.0);
            size.set(Some((w, h)));
        }
    };
    let on_resize_up = move |ev: PointerEvent| {
        resize_origin.set(None);
        if let Some(target) = ev
            .current_target()
            .and_then(|t| t.dyn_into::<Element>().ok())
        {
            let _ = target.release_pointer_capture(ev.pointer_id());
        }
    };

    let on_handle_down = move |ev: PointerEvent| {
        // Don't start a drag if the user clicked an interactive child
        // (the close button). Only grab drags that start on the header
        // element itself or the title span.
        if let Some(t) = ev.target().and_then(|t| t.dyn_into::<Element>().ok()) {
            let tag = t.tag_name();
            if tag.eq_ignore_ascii_case("button") {
                return;
            }
        }
        ev.prevent_default();
        if let Some(target) = ev
            .current_target()
            .and_then(|t| t.dyn_into::<Element>().ok())
        {
            let _ = target.set_pointer_capture(ev.pointer_id());
        }
        let (ox, oy) = offset.get_untracked();
        drag_origin.set(Some((ev.client_x() as f64, ev.client_y() as f64, ox, oy)));
    };
    let on_handle_move = move |ev: PointerEvent| {
        if let Some((sx, sy, ox, oy)) = drag_origin.get_untracked() {
            offset.set((
                ox + ev.client_x() as f64 - sx,
                oy + ev.client_y() as f64 - sy,
            ));
        }
    };
    let on_handle_up = move |ev: PointerEvent| {
        drag_origin.set(None);
        if let Some(target) = ev
            .current_target()
            .and_then(|t| t.dyn_into::<Element>().ok())
        {
            let _ = target.release_pointer_capture(ev.pointer_id());
        }
    };

    view! {
        <aside
            class="panel"
            aria-label="Feedback"
            style:transform=move || {
                let (x, y) = offset.get();
                format!("translate({x}px, {y}px)")
            }
            style:width=move || size.get().map(|(w, _)| format!("{w}px")).unwrap_or_default()
            style:height=move || size.get().map(|(_, h)| format!("{h}px")).unwrap_or_default()
            style:bottom=move || if size.get().is_some() { "auto".to_string() } else { String::new() }
        >
            <div
                class="panel-header panel-drag-handle"
                title="Drag to move"
                on:pointerdown=on_handle_down
                on:pointermove=on_handle_move
                on:pointerup=on_handle_up
                on:pointercancel=on_handle_up
            >
                <div class="panel-title">"Feedback"</div>
                <button
                    class="panel-close-btn"
                    title="Close"
                    aria-label="Close feedback panel"
                    on:click=move |_| is_open.set(false)
                >
                    "×"
                </button>
            </div>

            <div class="panel-body">
                <div class="panel-section-label">"Type"</div>
                <div class="type-chips">
                    {FeedbackType::all().into_iter().map(|ft| {
                        let is_active = move || feedback_type.get() == ft;
                        view! {
                            <button
                                class="type-chip"
                                class:active=is_active
                                on:click=move |_| feedback_type.set(ft)
                            >
                                {ft.label()}
                            </button>
                        }
                    }).collect_view()}
                </div>

                <div class="panel-section-label">"Describe it"</div>
                <textarea
                    class="text-area"
                    placeholder="What happened? What did you expect?"
                    prop:value=move || text.get()
                    on:input=move |ev| {
                        let el = event_target::<HtmlTextAreaElement>(&ev);
                        text.set(el.value());
                    }
                ></textarea>

                <div class="panel-section-label">"Voice"</div>
                <VoiceRecorder state=voice_view />

                {move || {
                    let thumbs = thumbs_ctrl.pasted_base64.get();
                    if thumbs.is_empty() {
                        view! { <span></span> }.into_any()
                    } else {
                        let count = thumbs.len();
                        view! {
                            <div>
                                <div class="panel-section-label">{format!("Attached images ({count})")}</div>
                                <div class="paste-thumbs">
                                    {thumbs.into_iter().map(|b64| {
                                        let src = format!("data:image/png;base64,{b64}");
                                        view! { <img class="paste-thumb" src=src /> }
                                    }).collect_view()}
                                </div>
                            </div>
                        }.into_any()
                    }
                }}
            </div>

            <div class="panel-footer">
                {move || match status.get() {
                    Some(Ok(msg)) => view!{ <div class="status-line success">{msg}</div> }.into_any(),
                    Some(Err(err)) => view!{ <div class="status-line error">{err}</div> }.into_any(),
                    None => view!{ <span></span> }.into_any(),
                }}
                <button
                    class="primary-btn"
                    prop:disabled=move || {
                        submitting.get()
                            || (text.get().trim().is_empty()
                                && disabled_ctrl.shapes.get().is_empty()
                                && disabled_ctrl.pasted_base64.get().is_empty())
                    }
                    on:click=submit
                >
                    {move || if submitting.get() { "Sending…" } else { "Submit to lineage" }}
                </button>
            </div>
            <div
                class="panel-resize-handle"
                title="Drag to resize"
                aria-label="Resize feedback panel"
                role="separator"
                on:pointerdown=on_resize_down
                on:pointermove=on_resize_move
                on:pointerup=on_resize_up
                on:pointercancel=on_resize_up
            ></div>
        </aside>
    }
}

/// Current on-screen dimensions of any existing `.panel` element. Used as
/// the starting point the first time the user grabs the resize handle so the
/// panel doesn't jump from its CSS-driven stretched height to a fixed value.
fn current_panel_size() -> (f64, f64) {
    let Some(doc) = window().and_then(|w| w.document()) else {
        return (320.0, 480.0);
    };
    let Some(el) = doc.query_selector(".panel").ok().flatten() else {
        return (320.0, 480.0);
    };
    let rect = el.get_bounding_client_rect();
    (rect.width().max(260.0), rect.height().max(240.0))
}

/// Tracks elements we forced to `visibility: hidden` while a capture is in
/// flight, so `restore_overlays` can put them back without touching anything
/// else in the DOM.
struct HiddenHandles {
    nodes: Vec<(HtmlElement, String)>,
}

fn hide_overlays_for_capture() -> HiddenHandles {
    let mut nodes = Vec::new();
    let Some(doc) = window().and_then(|w| w.document()) else {
        return HiddenHandles { nodes };
    };
    // Hide the canvas overlay (transparent annotation layer) and this feedback
    // panel itself so the captured screenshot shows the page beneath them.
    for sel in [".canvas-overlay", ".panel"] {
        if let Ok(Some(node)) = doc.query_selector(sel) {
            if let Ok(el) = node.dyn_into::<HtmlElement>() {
                let style = el.style();
                let prior = style.get_property_value("visibility").unwrap_or_default();
                let _ = style.set_property("visibility", "hidden");
                nodes.push((el, prior));
            }
        }
    }
    HiddenHandles { nodes }
}

fn restore_overlays(h: &HiddenHandles) {
    for (el, prior) in &h.nodes {
        let style = el.style();
        if prior.is_empty() {
            let _ = style.remove_property("visibility");
        } else {
            let _ = style.set_property("visibility", prior);
        }
    }
}

/// Double-RAF: the first frame applies the visibility change, the second
/// guarantees the compositor has painted the updated frame before we take
/// the native screenshot.
async fn wait_two_frames() {
    for _ in 0..2 {
        let Some(win) = window() else { return };
        let promise = js_sys::Promise::new(&mut |resolve, _| {
            let cb = Closure::once_into_js(move || {
                let _ = resolve.call0(&JsValue::UNDEFINED);
            });
            let _ = win.request_animation_frame(cb.as_ref().unchecked_ref());
        });
        let _ = JsFuture::from(promise).await;
    }
}

/// Draw the page PNG and the annotation PNG into an offscreen canvas and
/// return the resulting base64. The annotation is drawn only over the canvas
/// element's actual bounding rect in the captured image — not stretched to
/// the full page — so strokes stay aligned with the page content they're
/// annotating (the app bar stays visible, strokes don't creep over it).
async fn composite_page_and_annotations(page_b64: &str, anno_data_url: &str) -> Option<String> {
    let page_url = format!("data:image/png;base64,{page_b64}");
    let page_img = load_image(&page_url).await?;
    let anno_img = load_image(anno_data_url).await?;
    let pw = page_img.natural_width().max(1);
    let ph = page_img.natural_height().max(1);

    let doc = window().and_then(|w| w.document())?;
    let canvas: HtmlCanvasElement = doc
        .create_element("canvas")
        .ok()?
        .dyn_into::<HtmlCanvasElement>()
        .ok()?;
    canvas.set_width(pw);
    canvas.set_height(ph);
    let ctx: CanvasRenderingContext2d = canvas.get_context("2d").ok().flatten()?.dyn_into().ok()?;

    ctx.draw_image_with_html_image_element(&page_img, 0.0, 0.0)
        .ok()?;

    // Map the canvas-surface's CSS-pixel rect onto the captured image.
    // The capture may include native window chrome (title bar on macOS) so
    // the capture's height often exceeds `window.innerHeight * dpr` by the
    // chrome height. We scale by width ratio and offset vertically by that
    // chrome band.
    let win = window()?;
    let inner_w = win
        .inner_width()
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(1.0)
        .max(1.0);
    let inner_h = win
        .inner_height()
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(1.0)
        .max(1.0);
    let (dx, dy, dw, dh) = {
        let surface = doc
            .query_selector(".canvas-surface")
            .ok()
            .flatten()
            .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok());
        let rect = surface.as_ref().map(|el| el.get_bounding_client_rect());
        let scale_x = pw as f64 / inner_w;
        let scale_y = pw as f64 / inner_w; // preserve aspect using width ratio
        let chrome_offset = (ph as f64 - inner_h * scale_y).max(0.0);
        match rect {
            Some(r) => (
                r.left() * scale_x,
                chrome_offset + r.top() * scale_y,
                r.width() * scale_x,
                r.height() * scale_y,
            ),
            None => (0.0, 0.0, pw as f64, ph as f64),
        }
    };

    ctx.draw_image_with_html_image_element_and_dw_and_dh(&anno_img, dx, dy, dw, dh)
        .ok()?;

    let url = canvas.to_data_url_with_type("image/png").ok()?;
    url.split(',').nth(1).map(str::to_string)
}

async fn load_image(src: &str) -> Option<HtmlImageElement> {
    let img = HtmlImageElement::new().ok()?;
    img.set_src(src);
    let _ = JsFuture::from(img.decode()).await;
    if img.natural_width() == 0 {
        return None;
    }
    Some(img)
}

fn window_size() -> (u32, u32) {
    let Some(win) = window() else {
        return (0, 0);
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
    (w.max(0.0) as u32, h.max(0.0) as u32)
}
