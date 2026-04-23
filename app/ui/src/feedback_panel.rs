use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;
use web_sys::{window, Element, HtmlTextAreaElement, PointerEvent};

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
            let screenshot = ctrl
                .to_png_data_url()
                .and_then(|url| url.split(',').nth(1).map(str::to_string));
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
            let payload = SubmitFeedbackPayload {
                feedback_type: feedback_type.get_untracked(),
                page_route: route.get_untracked(),
                feedback_text: text.get_untracked(),
                annotations,
                pasted_images_base64: pasted,
                screenshot_base64: screenshot,
                voice_base64: voice.base64.get_untracked(),
                voice_mime_type: voice.mime_type.get_untracked(),
                voice_transcript: transcript,
                window_width: win_w,
                window_height: win_h,
            };

            let ctrl_reset = ctrl.clone();
            let voice_reset = voice.clone();
            spawn_local(async move {
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
        if let Some(target) = ev.current_target().and_then(|t| t.dyn_into::<Element>().ok()) {
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
        if let Some(target) = ev.current_target().and_then(|t| t.dyn_into::<Element>().ok()) {
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
        </aside>
    }
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
