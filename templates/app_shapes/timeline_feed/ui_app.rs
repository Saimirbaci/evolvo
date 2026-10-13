//! timeline_feed — UI scaffold (the NewApp content area).
//!
//! Drop into `app/ui/src/app_shape_timeline_feed.rs`, declare the new
//! module in `app/ui/src/main.rs`, and rewrite `app/ui/src/app.rs` so
//! its `App` component wraps `<TimelineFeedPage/>` inside `<Shell>`.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{json, Value as Json};
use wasm_bindgen::JsCast;

use crate::interop;
use crate::types::Event;

fn known_kinds() -> &'static [&'static str] {
    // FILL: event kind palette — the kinds the user's domain produces.
    // Used by the filter `<select>`. Free text in `record_event` still
    // works for kinds outside this list.
    &["info", "warn", "error", "deploy", "alert"]
}

#[component]
pub fn TimelineFeedPage() -> impl IntoView {
    let events: RwSignal<Vec<Event>> = RwSignal::new(Vec::new());
    let load_error: RwSignal<Option<String>> = RwSignal::new(None);
    let kind_filter: RwSignal<String> = RwSignal::new(String::new());
    let text_filter: RwSignal<String> = RwSignal::new(String::new());

    // Manual record form (the default UX; the webhook_ingest seam fills
    // events automatically when bound).
    let new_kind: RwSignal<String> = RwSignal::new("info".into());
    let new_summary: RwSignal<String> = RwSignal::new(String::new());

    let reload: RwSignal<u32> = RwSignal::new(0);
    Effect::new(move |_| {
        let _ = reload.get();
        let kinds = if kind_filter.get().is_empty() {
            Vec::<String>::new()
        } else {
            vec![kind_filter.get()]
        };
        let q = text_filter.get();
        spawn_local(async move {
            match interop::list_events(kinds, q).await {
                Ok(es) => events.set(es),
                Err(e) => load_error.set(Some(e)),
            }
        });
    });

    // FILL: live tail polling — kick `reload` every N seconds via
    // `set_interval`, plus an `on_cleanup` to clear it when the page
    // unmounts. See shell.rs::spawn_interval for the pattern.

    let record = move || {
        let kind = new_kind.get_untracked();
        let summary = new_summary.get_untracked().trim().to_string();
        if summary.is_empty() {
            return;
        }
        spawn_local(async move {
            match interop::record_event(&kind, &summary, json!({})).await {
                Ok(_) => {
                    new_summary.set(String::new());
                    reload.update(|v| *v = v.wrapping_add(1));
                }
                Err(e) => load_error.set(Some(e)),
            }
        });
    };

    let do_delete = move |id: String| {
        spawn_local(async move {
            match interop::delete_event(&id).await {
                Ok(_) => reload.update(|v| *v = v.wrapping_add(1)),
                Err(e) => load_error.set(Some(e)),
            }
        });
    };

    view! {
        <div class="feed-page">
            <header class="feed-toolbar">
                <select
                    class="feed-input feed-kind-filter"
                    on:change=move |ev| {
                        let t = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlSelectElement>().ok());
                        if let Some(t) = t { kind_filter.set(t.value()); }
                    }
                    aria-label="Filter by kind"
                >
                    <option value="">"All kinds"</option>
                    {known_kinds().iter().map(|k| view! {
                        <option value=k.to_string()>{k.to_string()}</option>
                    }).collect_view()}
                </select>
                <input
                    class="feed-input"
                    placeholder="Search…"
                    type="text"
                    prop:value=move || text_filter.get()
                    on:input=move |ev| {
                        let t = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                        if let Some(t) = t { text_filter.set(t.value()); }
                    }
                    aria-label="Search events"
                />
            </header>

            <header class="feed-record">
                <select
                    class="feed-input"
                    on:change=move |ev| {
                        let t = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlSelectElement>().ok());
                        if let Some(t) = t { new_kind.set(t.value()); }
                    }
                    aria-label="New event kind"
                >
                    {known_kinds().iter().map(|k| view! {
                        <option value=k.to_string()>{k.to_string()}</option>
                    }).collect_view()}
                </select>
                <input
                    class="feed-input feed-record-summary"
                    placeholder="Summary"
                    type="text"
                    prop:value=move || new_summary.get()
                    on:input=move |ev| {
                        let t = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                        if let Some(t) = t { new_summary.set(t.value()); }
                    }
                    on:keydown=move |ev| {
                        if ev.key() == "Enter" {
                            ev.prevent_default();
                            record.clone()();
                        }
                    }
                    aria-label="New event summary"
                />
                <button
                    class="primary-btn"
                    on:click=move |_| record.clone()()
                    prop:disabled=move || new_summary.get().trim().is_empty()
                    aria-label="Record event"
                    title="Record a new event"
                >
                    "Record"
                </button>
            </header>

            {move || match load_error.get() {
                Some(e) => view! {
                    <div class="feed-error" role="alert">{e}</div>
                }.into_any(),
                None => view! { <span></span> }.into_any(),
            }}

            <ol class="feed-list">
                {move || {
                    let es = events.get();
                    if es.is_empty() {
                        return view! {
                            <li class="empty-state">"No events match."</li>
                        }.into_any();
                    }
                    let mut current_day: Option<String> = None;
                    let mut nodes = Vec::new();
                    for e in es {
                        let day = day_of(e.occurred_at);
                        if Some(&day) != current_day.as_ref() {
                            current_day = Some(day.clone());
                            nodes.push(view! {
                                <li class="feed-day-divider">{day}</li>
                            }.into_any());
                        }
                        let id = e.id.clone();
                        let kind_class = format!("feed-row feed-row-{}", e.kind);
                        let payload_str = if e.payload.is_null() {
                            String::new()
                        } else {
                            serde_json::to_string_pretty(&e.payload).unwrap_or_default()
                        };
                        let do_delete = do_delete.clone();
                        nodes.push(view! {
                            <li class=kind_class>
                                <span class="feed-row-time">{format_hms(e.occurred_at)}</span>
                                // FILL: per-kind icon — render an icon glyph
                                // before the kind label.
                                <span class="feed-row-kind">{e.kind.clone()}</span>
                                <span class="feed-row-summary">{e.summary.clone()}</span>
                                {if payload_str.is_empty() {
                                    view! { <span></span> }.into_any()
                                } else {
                                    view! {
                                        // FILL: payload preview rendering — replace
                                        // <pre> with a richer view (copyable, syntax-
                                        // highlighted, key-value table) when it helps.
                                        <details class="feed-row-payload">
                                            <summary>"payload"</summary>
                                            <pre>{payload_str}</pre>
                                        </details>
                                    }.into_any()
                                }}
                                <button
                                    class="feed-row-delete"
                                    on:click=move |_| do_delete(id.clone())
                                    aria-label="Delete event"
                                    title="Delete this event"
                                >
                                    "×"
                                </button>
                            </li>
                        }.into_any());
                    }
                    nodes.into_iter().collect_view().into_any()
                }}
            </ol>
        </div>
    }
}

#[allow(dead_code)]
fn _example() -> Json {
    json!({})
}

fn format_hms(ms: u64) -> String {
    if ms == 0 {
        return String::new();
    }
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms as f64));
    let s: String = date.to_iso_string().into();
    if s.len() >= 19 {
        s[11..19].to_string()
    } else {
        s
    }
}

fn day_of(ms: u64) -> String {
    if ms == 0 {
        return "—".into();
    }
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms as f64));
    let s: String = date.to_iso_string().into();
    if s.len() >= 10 {
        s[..10].to_string()
    } else {
        s
    }
}
