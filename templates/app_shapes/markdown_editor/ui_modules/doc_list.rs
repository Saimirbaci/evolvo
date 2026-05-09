//! markdown_editor — sidebar listing the user's documents.
//!
//! Drop into `app/ui/src/markdown_editor_doc_list.rs`. Declared from
//! `app_shape_markdown_editor.rs`'s imports; declared as a module in
//! `app/ui/src/main.rs`.

use leptos::prelude::*;

use crate::types::Document;

#[component]
pub fn DocList(
    documents: RwSignal<Vec<Document>>,
    selected_id: RwSignal<Option<String>>,
    /// Callback invoked when the user clicks "New". Owns its own state in
    /// the parent (clears title + body), so this component just fires it.
    on_new: Box<dyn Fn() + 'static>,
) -> impl IntoView {
    view! {
        <aside class="md-editor-sidebar">
            <header class="md-editor-sidebar-head">
                <h2 class="md-editor-sidebar-title">"Documents"</h2>
                <button
                    class="primary-btn md-editor-new-btn"
                    on:click=move |_| on_new()
                    aria-label="New document"
                    title="Start a new blank document"
                >
                    "+ New"
                </button>
            </header>

            // FILL: list-side affordances — filtering, grouping, sorting
            // controls. Bind to additional signals on the parent or
            // recompute a derived `Memo` of `documents` here.

            {move || {
                let docs = documents.get();
                if docs.is_empty() {
                    return view! {
                        // FILL: empty-state copy — the message shown when
                        // the user has not created any documents yet.
                        <div class="md-editor-empty">
                            "No documents yet. Click + New to start one."
                        </div>
                    }.into_any();
                }
                view! {
                    <ul class="md-editor-doc-list">
                        {docs.into_iter().map(|d| {
                            let id = d.id.clone();
                            let id_for_click = id.clone();
                            let id_for_active = id.clone();
                            let title_display = if d.title.trim().is_empty() {
                                "(untitled)".to_string()
                            } else {
                                d.title.clone()
                            };
                            let is_active = move || {
                                selected_id.get().as_deref() == Some(id_for_active.as_str())
                            };
                            view! {
                                <li>
                                    <button
                                        class="md-editor-doc-list-item"
                                        class:active=is_active
                                        on:click=move |_| selected_id.set(Some(id_for_click.clone()))
                                    >
                                        <span class="md-editor-doc-title">{title_display}</span>
                                        <span class="md-editor-doc-meta">{format_updated(d.updated_at)}</span>
                                    </button>
                                </li>
                            }
                        }).collect_view()}
                    </ul>
                }.into_any()
            }}
        </aside>
    }
}

fn format_updated(unix_ms: u64) -> String {
    if unix_ms == 0 {
        return String::new();
    }
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(unix_ms as f64));
    let s: String = date.to_iso_string().into();
    // Trim to YYYY-MM-DD HH:MM for readability.
    if s.len() >= 16 {
        s[..16].replace('T', " ")
    } else {
        s
    }
}
