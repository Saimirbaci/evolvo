//! "Design" bar at the top of the feedback panel: save the current canvas
//! (every storyboard screen + description + type) as a named design, reopen
//! or delete saved designs, and keep an autosaved draft so closing the app
//! never loses a sketch.
//!
//! Tauri-agnostic apart from the `interop` calls; all host logic (ids,
//! validation, image storage) lives in `app/src-tauri/src/designs.rs`.

use std::time::Duration;

use leptos::prelude::*;
use leptos::task::spawn_local;
use web_sys::HtmlInputElement;

use crate::canvas::CanvasController;
use crate::feedback_panel::{load_active_screen, window_size, StoryboardScreenDraft};
use crate::interop;
use crate::types::{
    DesignSummary, EditableDesign, FeedbackRecord, FeedbackType, SaveDesignPayload,
    SaveDesignScreenPayload,
};

/// Quiet period after the last edit before the draft is written.
const AUTOSAVE_DEBOUNCE: Duration = Duration::from_millis(1500);
const THUMBNAIL_WIDTH: f64 = 320.0;

/// The feedback-panel state a design captures and restores.
#[derive(Clone, Copy)]
pub struct DesignBindings {
    pub screens: RwSignal<Vec<StoryboardScreenDraft>>,
    pub active_screen: RwSignal<usize>,
    pub next_screen_number: RwSignal<usize>,
    pub text: RwSignal<String>,
    pub feedback_type: RwSignal<FeedbackType>,
    pub route: RwSignal<String>,
    pub revision_parent: RwSignal<Option<FeedbackRecord>>,
}

/// Snapshot every screen, reading the active one live from the canvas so
/// unsaved strokes are included without writing back into `screens` (which
/// would re-trigger the autosave effect).
fn screen_payloads(ctrl: &CanvasController, b: DesignBindings) -> Vec<SaveDesignScreenPayload> {
    let (win_w, win_h) = window_size();
    let active = b.active_screen.get_untracked();
    let live_shapes = ctrl.shapes.get_untracked();
    b.screens.with_untracked(|screens| {
        screens
            .iter()
            .enumerate()
            .map(|(idx, screen)| {
                let shapes = if idx == active {
                    &live_shapes
                } else {
                    &screen.shapes
                };
                SaveDesignScreenPayload {
                    id: screen.id.clone(),
                    name: screen.name.clone(),
                    description: screen.description.clone(),
                    intended_route: screen.intended_route.clone(),
                    annotations: CanvasController::annotations_for_shapes(shapes),
                    pasted_images_base64: CanvasController::pasted_base64_for_shapes(shapes),
                    window_width: win_w,
                    window_height: win_h,
                }
            })
            .collect()
    })
}

/// Cheap content fingerprint for dirty tracking: vector annotations, text
/// fields and image sizes — not the image bytes themselves.
fn fingerprint(b: DesignBindings, screens: &[SaveDesignScreenPayload]) -> String {
    let shape_part: Vec<_> = screens
        .iter()
        .map(|s| {
            (
                &s.name,
                &s.description,
                &s.intended_route,
                &s.annotations,
                s.pasted_images_base64
                    .iter()
                    .map(String::len)
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    serde_json::to_string(&(
        format!("{:?}", b.feedback_type.get_untracked()),
        b.text.get_untracked(),
        shape_part,
    ))
    .unwrap_or_default()
}

fn has_content(b: DesignBindings, screens: &[SaveDesignScreenPayload]) -> bool {
    !b.text.get_untracked().trim().is_empty()
        || screens
            .iter()
            .any(|s| !s.annotations.is_empty() || !s.description.trim().is_empty())
}

/// Replace the panel + canvas with a loaded design.
fn apply_design(ctrl: &CanvasController, b: DesignBindings, design: EditableDesign) {
    let drafts: Vec<StoryboardScreenDraft> = design
        .screens
        .into_iter()
        .map(StoryboardScreenDraft::from_editable)
        .collect();
    let drafts = if drafts.is_empty() {
        vec![StoryboardScreenDraft::empty(1, b.route.get_untracked())]
    } else {
        drafts
    };
    if let Some(ft) = design.feedback_type {
        b.feedback_type.set(ft);
    }
    b.text.set(design.feedback_text);
    b.revision_parent.set(None);
    b.next_screen_number.set(drafts.len().saturating_add(1));
    b.active_screen.set(0);
    b.screens.set(drafts.clone());
    load_active_screen(ctrl, &drafts[0]);
}

fn start_blank(ctrl: &CanvasController, b: DesignBindings) {
    ctrl.reset();
    b.screens.set(vec![StoryboardScreenDraft::empty(
        1,
        b.route.get_untracked(),
    )]);
    b.active_screen.set(0);
    b.next_screen_number.set(2);
    b.text.set(String::new());
    b.revision_parent.set(None);
}

fn relative_time(unix_ms: u64) -> String {
    let now = js_sys::Date::now() as u64;
    let secs = now.saturating_sub(unix_ms) / 1000;
    match secs {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86_399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

/// A pending destructive click awaiting its confirming second click.
#[derive(Clone, PartialEq)]
enum Confirm {
    New,
    Open(String),
    Delete(String),
}

#[component]
pub fn DesignBar(controller: CanvasController, bindings: DesignBindings) -> impl IntoView {
    let b = bindings;
    let current: RwSignal<Option<DesignSummary>> = RwSignal::new(None);
    let name = RwSignal::new(String::new());
    // Fingerprint at the last manual save / load — drives "unsaved".
    let saved_fp: RwSignal<Option<String>> = RwSignal::new(None);
    let live_fp = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let message: RwSignal<Option<Result<String, String>>> = RwSignal::new(None);
    let list_open = RwSignal::new(false);
    let designs: RwSignal<Option<Result<Vec<DesignSummary>, String>>> = RwSignal::new(None);
    let confirm: RwSignal<Option<Confirm>> = RwSignal::new(None);
    // Autosave bookkeeping: nothing is written until the startup draft
    // check finished, so an empty initial canvas never clobbers the draft.
    let hydrated = RwSignal::new(false);
    let autosave_token = StoredValue::new(0u64);
    let last_draft_fp: StoredValue<Option<String>> = StoredValue::new(None);
    // Whether a draft is known to exist on disk. Clearing is only allowed
    // when it does AND the startup load succeeded — a failed load must never
    // turn into deleting the user's autosaved sketch.
    let draft_on_disk = StoredValue::new(false);
    let may_clear_draft = StoredValue::new(false);

    let dirty = move || {
        let live = live_fp.get();
        match saved_fp.get() {
            Some(saved) => saved != live,
            None => !live.is_empty(),
        }
    };

    let refresh_list = move || {
        spawn_local(async move {
            designs.set(Some(interop::list_designs().await));
        });
    };

    // Restore the autosaved draft once, if the panel is still blank.
    {
        let ctrl = controller.clone();
        Effect::new(move |already: Option<()>| {
            if already.is_some() {
                return;
            }
            let ctrl = ctrl.clone();
            spawn_local(async move {
                let loaded = interop::load_design_draft().await;
                may_clear_draft.set_value(loaded.is_ok());
                draft_on_disk.set_value(!matches!(loaded, Ok(None)));
                if let Ok(Some(draft)) = loaded {
                    let payloads = screen_payloads(&ctrl, b);
                    let blank =
                        !has_content(b, &payloads) && b.revision_parent.get_untracked().is_none();
                    if blank {
                        let source = draft.source_design_id.clone();
                        let draft_name = draft.summary.name.clone();
                        apply_design(&ctrl, b, draft);
                        name.set(draft_name);
                        if let Some(id) = source {
                            // Reconnect to the saved design so Save updates it;
                            // it stays "unsaved" until the user saves.
                            if let Ok(Some(saved)) = interop::load_design(&id).await {
                                current.set(Some(saved.summary));
                            }
                        }
                        message.set(Some(Ok(
                            "Restored your unsaved sketch from last time.".into()
                        )));
                    }
                }
                last_draft_fp.set_value(None);
                hydrated.set(true);
            });
        });
    }

    // Track content changes: update the dirty fingerprint and debounce an
    // autosave of the draft. Empty content clears the draft instead, so a
    // submit / "New" doesn't resurrect old work next launch.
    {
        let ctrl = controller.clone();
        Effect::new(move |_| {
            ctrl.shapes.track();
            b.screens.track();
            b.text.track();
            b.feedback_type.track();
            b.active_screen.track();
            if !hydrated.get() {
                return;
            }
            let payloads = screen_payloads(&ctrl, b);
            let fp = fingerprint(b, &payloads);
            let empty = !has_content(b, &payloads);
            live_fp.set(if empty && current.get_untracked().is_none() {
                String::new()
            } else {
                fp.clone()
            });
            if last_draft_fp.get_value().as_deref() == Some(fp.as_str()) {
                return;
            }
            if empty && !(draft_on_disk.get_value() && may_clear_draft.get_value()) {
                // Nothing to save and nothing (safely) to clear.
                last_draft_fp.set_value(Some(fp));
                return;
            }
            let token = autosave_token.get_value().wrapping_add(1);
            autosave_token.set_value(token);
            let ctrl = ctrl.clone();
            set_timeout(
                move || {
                    if autosave_token.try_get_value() != Some(token) {
                        return; // superseded by a newer edit
                    }
                    let payloads = screen_payloads(&ctrl, b);
                    let fp = fingerprint(b, &payloads);
                    let empty = !has_content(b, &payloads);
                    if empty && !(draft_on_disk.get_value() && may_clear_draft.get_value()) {
                        return;
                    }
                    let payload = SaveDesignPayload {
                        id: None,
                        name: name.get_untracked(),
                        feedback_type: Some(b.feedback_type.get_untracked()),
                        feedback_text: b.text.get_untracked(),
                        screens: payloads,
                        thumbnail_png_base64: None,
                        source_design_id: current.get_untracked().map(|d| d.id),
                    };
                    spawn_local(async move {
                        let result = if empty {
                            interop::clear_design_draft().await.map(|_| ())
                        } else {
                            interop::save_design_draft(&payload).await.map(|_| ())
                        };
                        match result {
                            Ok(()) => {
                                last_draft_fp.set_value(Some(fp));
                                draft_on_disk.set_value(!empty);
                                // After a successful write we know the state.
                                may_clear_draft.set_value(true);
                            }
                            Err(err) => {
                                message.try_set(Some(Err(format!("Autosave failed: {err}"))));
                            }
                        }
                    });
                },
                AUTOSAVE_DEBOUNCE,
            );
        });
    }

    let save = {
        let ctrl = controller.clone();
        move |as_new: bool| {
            if busy.get_untracked() {
                return;
            }
            let payloads = screen_payloads(&ctrl, b);
            if !has_content(b, &payloads) {
                message.set(Some(Err(
                    "Nothing to save yet — draw or describe something first.".into(),
                )));
                return;
            }
            let fp = fingerprint(b, &payloads);
            let payload = SaveDesignPayload {
                id: if as_new {
                    None
                } else {
                    current.get_untracked().map(|d| d.id)
                },
                name: name.get_untracked(),
                feedback_type: Some(b.feedback_type.get_untracked()),
                feedback_text: b.text.get_untracked(),
                screens: payloads,
                thumbnail_png_base64: ctrl.thumbnail_png_base64(THUMBNAIL_WIDTH),
                source_design_id: None,
            };
            busy.set(true);
            message.set(None);
            spawn_local(async move {
                match interop::save_design(&payload).await {
                    Ok(summary) => {
                        name.try_set(summary.name.clone());
                        message.try_set(Some(Ok(format!("Saved “{}”", summary.name))));
                        current.try_set(Some(summary));
                        saved_fp.try_set(Some(fp));
                        if list_open.get_untracked() {
                            refresh_list();
                        }
                    }
                    Err(err) => {
                        message.try_set(Some(Err(format!("Could not save: {err}"))));
                    }
                }
                busy.try_set(false);
            });
        }
    };

    let open_design = {
        let ctrl = controller.clone();
        move |id: String| {
            if dirty() && confirm.get_untracked() != Some(Confirm::Open(id.clone())) {
                confirm.set(Some(Confirm::Open(id)));
                return;
            }
            confirm.set(None);
            busy.set(true);
            let ctrl = ctrl.clone();
            spawn_local(async move {
                match interop::load_design(&id).await {
                    Ok(Some(design)) => {
                        let summary = design.summary.clone();
                        apply_design(&ctrl, b, design);
                        name.set(summary.name.clone());
                        // Fingerprint the freshly loaded state as "saved".
                        let payloads = screen_payloads(&ctrl, b);
                        saved_fp.set(Some(fingerprint(b, &payloads)));
                        message.set(Some(Ok(format!("Opened “{}”", summary.name))));
                        current.set(Some(summary));
                        list_open.set(false);
                    }
                    Ok(None) => {
                        message.set(Some(Err("That design no longer exists.".into())));
                        refresh_list();
                    }
                    Err(err) => message.set(Some(Err(format!("Could not open: {err}")))),
                }
                busy.set(false);
            });
        }
    };

    let delete_design = move |id: String| {
        if confirm.get_untracked() != Some(Confirm::Delete(id.clone())) {
            confirm.set(Some(Confirm::Delete(id)));
            return;
        }
        confirm.set(None);
        spawn_local(async move {
            match interop::delete_design(&id).await {
                Ok(_) => {
                    if current.get_untracked().is_some_and(|d| d.id == id) {
                        current.set(None);
                        saved_fp.set(None);
                    }
                    message.set(Some(Ok("Design deleted.".into())));
                }
                Err(err) => message.set(Some(Err(format!("Could not delete: {err}")))),
            }
            refresh_list();
        });
    };

    let new_design = {
        let ctrl = controller.clone();
        move || {
            if dirty() && confirm.get_untracked() != Some(Confirm::New) {
                confirm.set(Some(Confirm::New));
                return;
            }
            confirm.set(None);
            start_blank(&ctrl, b);
            current.set(None);
            saved_fp.set(None);
            name.set(String::new());
            message.set(None);
        }
    };

    view! {
        <section class="design-bar" aria-label="Design">
            <div class="design-row">
                <input
                    class="input design-name"
                    type="text"
                    placeholder="Untitled design"
                    aria-label="Design name"
                    maxlength="80"
                    prop:value=move || name.get()
                    on:input=move |ev| {
                        name.set(event_target::<HtmlInputElement>(&ev).value());
                    }
                />
                {
                    let save = save.clone();
                    view! {
                        <button
                            class="btn btn-primary design-btn"
                            title=move || if current.get().is_some() { "Save changes to this design" } else { "Save as a new design" }
                            disabled=move || busy.get()
                            on:click=move |_| save(false)
                        >
                            "Save"
                        </button>
                    }
                }
                <button
                    class="btn btn-ghost design-btn"
                    title="Open a saved design"
                    aria-expanded=move || if list_open.get() { "true" } else { "false" }
                    on:click=move |_| {
                        let opening = !list_open.get_untracked();
                        list_open.set(opening);
                        confirm.set(None);
                        if opening {
                            designs.set(None);
                            refresh_list();
                        }
                    }
                >
                    "Open…"
                </button>
            </div>
            <div class="design-meta">
                <span class="design-state">
                    {move || match (current.get(), dirty()) {
                        (Some(_), true) => "Unsaved changes".to_string(),
                        (Some(d), false) => format!("Saved {}", relative_time(d.updated_at_unix_ms)),
                        (None, true) => "Not saved yet — autosaved as a draft".to_string(),
                        (None, false) => "New design".to_string(),
                    }}
                </span>
                {
                    let save = save.clone();
                    move || current.get().map(|_| {
                        let save = save.clone();
                        view! {
                            <button class="design-link" disabled=move || busy.get() on:click=move |_| save(true)>
                                "Save as new"
                            </button>
                        }
                    })
                }
                {
                    let new_design = new_design.clone();
                    view! {
                        <button class="design-link" on:click=move |_| new_design()>
                            {move || if confirm.get() == Some(Confirm::New) { "Discard changes & start new" } else { "New" }}
                        </button>
                    }
                }
            </div>
            {move || message.get().map(|m| match m {
                Ok(text) => view! { <div class="design-message ok" role="status">{text}</div> }.into_any(),
                Err(text) => view! { <div class="design-message error" role="alert">{text}</div> }.into_any(),
            })}
            {
                let open_design = open_design.clone();
                move || {
                    if !list_open.get() {
                        return None;
                    }
                    let open_design = open_design.clone();
                    let body = match designs.get() {
                        None => view! { <div class="design-empty">"Loading…"</div> }.into_any(),
                        Some(Err(err)) => view! { <div class="design-message error">{format!("Could not list designs: {err}")}</div> }.into_any(),
                        Some(Ok(list)) if list.is_empty() => view! {
                            <div class="design-empty">"No saved designs yet. Draw something and press Save."</div>
                        }.into_any(),
                        Some(Ok(list)) => view! {
                            <ul class="design-list">
                                {list.into_iter().map(|d| {
                                    let open = open_design.clone();
                                    let id_open = d.id.clone();
                                    let id_delete = d.id.clone();
                                    let id_confirm_open = d.id.clone();
                                    let id_confirm_delete = d.id.clone();
                                    let is_current = current.get_untracked().is_some_and(|c| c.id == d.id);
                                    let meta = format!(
                                        "{} screen{} · {} shapes · {}",
                                        d.screen_count,
                                        if d.screen_count == 1 { "" } else { "s" },
                                        d.shape_count,
                                        relative_time(d.updated_at_unix_ms)
                                    );
                                    view! {
                                        <li class="design-item" class:current=is_current>
                                            <span class="design-thumb" aria-hidden="true">
                                                {d.thumbnail_data_url.clone().map(|src| view! { <img src=src alt="" /> })}
                                            </span>
                                            <span class="design-item-copy">
                                                <strong>{d.name.clone()}</strong>
                                                <small>{meta}</small>
                                            </span>
                                            <button
                                                class="btn btn-ghost design-btn"
                                                title=format!("Open {}", d.name)
                                                on:click=move |_| open(id_open.clone())
                                            >
                                                {move || if confirm.get() == Some(Confirm::Open(id_confirm_open.clone())) { "Discard & open" } else { "Open" }}
                                            </button>
                                            <button
                                                class="btn btn-ghost design-btn design-delete"
                                                title=format!("Delete {}", d.name)
                                                aria-label=format!("Delete {}", d.name)
                                                on:click=move |_| delete_design(id_delete.clone())
                                            >
                                                {move || if confirm.get() == Some(Confirm::Delete(id_confirm_delete.clone())) { "Confirm" } else { "Delete" }}
                                            </button>
                                        </li>
                                    }
                                }).collect_view()}
                            </ul>
                        }.into_any(),
                    };
                    Some(view! { <div class="design-picker">{body}</div> })
                }
            }
        </section>
    }
}
