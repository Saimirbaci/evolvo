//! markdown_editor — UI scaffold (the NewApp content area).
//!
//! Drop into `app/ui/src/app_shape_markdown_editor.rs`, declare the new
//! module in `app/ui/src/main.rs`, and rewrite `app/ui/src/app.rs` so its
//! `App` component wraps `<MarkdownEditorPage/>` inside `<Shell>`. The
//! shell still owns the Feedback FAB, the Lineage page, and the Canvas
//! overlay — do not reimplement any of them here.
//!
//! Companion modules:
//!   - `markdown_editor_doc_list.rs` — the sidebar.
//!   - `markdown_editor_preview.rs`  — the read-only preview.
//!
//! The four backend Tauri commands (list_documents, load_document,
//! save_document, delete_document) are referenced via typed wrappers
//! added to `app/ui/src/interop.rs` during BackendImpl. Wire them up
//! exactly as the existing interop wrappers do (see app_health,
//! submit_feedback for the pattern).

use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;

use crate::interop;
use crate::markdown_editor_doc_list::DocList;
use crate::markdown_editor_preview::MarkdownPreview;
use crate::types::Document;

#[component]
pub fn MarkdownEditorPage() -> impl IntoView {
    let documents: RwSignal<Vec<Document>> = RwSignal::new(Vec::new());
    let selected_id: RwSignal<Option<String>> = RwSignal::new(None);
    let title: RwSignal<String> = RwSignal::new(String::new());
    let body: RwSignal<String> = RwSignal::new(String::new());
    let load_error: RwSignal<Option<String>> = RwSignal::new(None);
    let save_pending: RwSignal<bool> = RwSignal::new(false);

    // Initial load + reload trigger. Bumping `reload` re-fetches the list.
    let reload: RwSignal<u32> = RwSignal::new(0);
    Effect::new(move |_| {
        let _ = reload.get();
        spawn_local(async move {
            match interop::list_documents().await {
                Ok(docs) => {
                    documents.set(docs);
                }
                Err(e) => load_error.set(Some(e)),
            }
        });
    });

    // When the user selects a different doc, fetch its full body.
    Effect::new(move |prev: Option<Option<String>>| {
        let now = selected_id.get();
        if prev.as_ref() == Some(&now) {
            return now;
        }
        if let Some(id) = now.clone() {
            spawn_local(async move {
                match interop::load_document(&id).await {
                    Ok(doc) => {
                        title.set(doc.title);
                        body.set(doc.body);
                    }
                    Err(e) => load_error.set(Some(e)),
                }
            });
        } else {
            title.set(String::new());
            body.set(String::new());
        }
        now
    });

    let do_save = move || {
        if save_pending.get_untracked() {
            return;
        }
        save_pending.set(true);
        let id = selected_id.get_untracked();
        let payload_title = title.get_untracked();
        let payload_body = body.get_untracked();
        spawn_local(async move {
            match interop::save_document(id.as_deref(), &payload_title, &payload_body).await {
                Ok(saved) => {
                    selected_id.set(Some(saved.id));
                    reload.update(|v| *v = v.wrapping_add(1));
                }
                Err(e) => load_error.set(Some(e)),
            }
            save_pending.set(false);
        });
    };

    let do_new = move || {
        selected_id.set(None);
        title.set(String::new());
        body.set(String::new());
    };

    let do_delete = move || {
        let Some(id) = selected_id.get_untracked() else {
            return;
        };
        spawn_local(async move {
            match interop::delete_document(&id).await {
                Ok(_) => {
                    selected_id.set(None);
                    title.set(String::new());
                    body.set(String::new());
                    reload.update(|v| *v = v.wrapping_add(1));
                }
                Err(e) => load_error.set(Some(e)),
            }
        });
    };

    // FILL: keyboard shortcuts — bind ⌘S to do_save(), ⌘N to do_new(), etc.

    view! {
        <div class="md-editor-page">
            <DocList
                documents=documents
                selected_id=selected_id
                on_new=Box::new(do_new.clone())
            />
            <section class="md-editor-source">
                <header class="md-editor-toolbar">
                    <input
                        class="md-editor-title-input"
                        type="text"
                        placeholder="Title"
                        prop:value=move || title.get()
                        on:input=move |ev| {
                            let t = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                            if let Some(t) = t { title.set(t.value()); }
                        }
                        aria-label="Document title"
                    />
                    <div class="md-editor-toolbar-actions">
                        // FILL: domain-specific document actions — extra
                        // buttons (publish, export, …) wired to extra Tauri
                        // commands. Mirror the save / delete pattern.
                        <button
                            class="secondary-btn"
                            on:click=move |_| do_delete()
                            prop:disabled=move || selected_id.get().is_none()
                            aria-label="Delete the current document"
                            title="Delete the current document"
                        >
                            "Delete"
                        </button>
                        <button
                            class="primary-btn"
                            on:click=move |_| do_save()
                            prop:disabled=move || save_pending.get() || title.get().trim().is_empty()
                            aria-label="Save the current document"
                            title="Save (creates a new document if nothing is selected)"
                        >
                            {move || if save_pending.get() { "Saving…" } else { "Save" }}
                        </button>
                    </div>
                </header>
                <textarea
                    class="md-editor-source-area"
                    placeholder="Write markdown here…"
                    prop:value=move || body.get()
                    on:input=move |ev| {
                        let t = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlTextAreaElement>().ok());
                        if let Some(t) = t { body.set(t.value()); }
                    }
                    aria-label="Document body"
                ></textarea>
                {move || match load_error.get() {
                    Some(e) => view! {
                        <div class="md-editor-error" role="alert">{e}</div>
                    }.into_any(),
                    None => view! { <span></span> }.into_any(),
                }}
            </section>
            <MarkdownPreview body=body />
        </div>
    }
}
