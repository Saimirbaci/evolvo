//! dashboard — UI scaffold (the NewApp content area).
//!
//! Drop into `app/ui/src/app_shape_dashboard.rs`, declare the new module
//! in `app/ui/src/main.rs`, and rewrite `app/ui/src/app.rs` so its `App`
//! component wraps `<DashboardPage/>` inside `<Shell>`.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::Value as Json;
use wasm_bindgen::JsCast;

use crate::interop;
use crate::types::Widget;

#[component]
pub fn DashboardPage() -> impl IntoView {
    let widgets: RwSignal<Vec<Widget>> = RwSignal::new(Vec::new());
    let load_error: RwSignal<Option<String>> = RwSignal::new(None);
    let new_title: RwSignal<String> = RwSignal::new(String::new());
    let new_kind: RwSignal<String> = RwSignal::new("number".into());
    let new_url: RwSignal<String> = RwSignal::new(String::new());

    let reload: RwSignal<u32> = RwSignal::new(0);
    Effect::new(move |_| {
        let _ = reload.get();
        spawn_local(async move {
            match interop::list_widgets().await {
                Ok(ws) => widgets.set(ws),
                Err(e) => load_error.set(Some(e)),
            }
        });
    });

    let add_widget = move || {
        let title = new_title.get_untracked().trim().to_string();
        if title.is_empty() {
            return;
        }
        let kind = new_kind.get_untracked();
        let url = new_url.get_untracked().trim().to_string();
        let url_opt = if url.is_empty() { None } else { Some(url) };
        spawn_local(async move {
            match interop::save_widget(None, &title, &kind, url_opt.as_deref(), None).await {
                Ok(_) => {
                    new_title.set(String::new());
                    new_url.set(String::new());
                    reload.update(|v| *v = v.wrapping_add(1));
                }
                Err(e) => load_error.set(Some(e)),
            }
        });
    };

    let do_refresh = move |id: String| {
        spawn_local(async move {
            match interop::refresh_widget(&id).await {
                Ok(_) => reload.update(|v| *v = v.wrapping_add(1)),
                Err(e) => load_error.set(Some(e)),
            }
        });
    };

    let do_delete = move |id: String| {
        spawn_local(async move {
            match interop::delete_widget(&id).await {
                Ok(_) => reload.update(|v| *v = v.wrapping_add(1)),
                Err(e) => load_error.set(Some(e)),
            }
        });
    };

    view! {
        <div class="dashboard-page">
            <header class="dashboard-toolbar">
                <input
                    class="dashboard-input"
                    placeholder="Widget title"
                    type="text"
                    prop:value=move || new_title.get()
                    on:input=move |ev| {
                        let t = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                        if let Some(t) = t { new_title.set(t.value()); }
                    }
                />
                <select
                    class="dashboard-input"
                    on:change=move |ev| {
                        let t = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlSelectElement>().ok());
                        if let Some(t) = t { new_kind.set(t.value()); }
                    }
                >
                    <option value="number">"Number"</option>
                    <option value="text">"Text"</option>
                </select>
                <input
                    class="dashboard-input dashboard-url"
                    placeholder="Source URL (optional)"
                    type="text"
                    prop:value=move || new_url.get()
                    on:input=move |ev| {
                        let t = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                        if let Some(t) = t { new_url.set(t.value()); }
                    }
                />
                <button
                    class="primary-btn"
                    on:click=move |_| add_widget.clone()()
                    prop:disabled=move || new_title.get().trim().is_empty()
                    aria-label="Add widget"
                    title="Add a new widget"
                >
                    "+ Add widget"
                </button>
            </header>

            {move || match load_error.get() {
                Some(e) => view! {
                    <div class="dashboard-error" role="alert">{e}</div>
                }.into_any(),
                None => view! { <span></span> }.into_any(),
            }}

            <section class="dashboard-grid">
                {move || {
                    let ws = widgets.get();
                    if ws.is_empty() {
                        return view! {
                            <div class="empty-state">"No widgets yet."</div>
                        }.into_any();
                    }
                    ws.into_iter().map(|w| {
                        let id = w.id.clone();
                        let id_for_refresh = id.clone();
                        let id_for_delete = id.clone();
                        let title = w.title.clone();
                        let kind = w.kind.clone();
                        let value = w.latest_value.clone();
                        let fetched = w.latest_fetched_at;
                        let source = w.source_url.clone();
                        let do_refresh = do_refresh.clone();
                        let do_delete = do_delete.clone();
                        view! {
                            <article class="dashboard-tile">
                                <header class="dashboard-tile-head">
                                    <h3>{title}</h3>
                                    <div class="dashboard-tile-actions">
                                        <button
                                            class="dashboard-tile-btn"
                                            on:click=move |_| do_refresh(id_for_refresh.clone())
                                            aria-label="Refresh widget"
                                            title="Refresh now"
                                        >
                                            "↻"
                                        </button>
                                        <button
                                            class="dashboard-tile-btn dashboard-tile-btn-danger"
                                            on:click=move |_| do_delete(id_for_delete.clone())
                                            aria-label="Delete widget"
                                            title="Delete"
                                        >
                                            "×"
                                        </button>
                                    </div>
                                </header>
                                <div class="dashboard-tile-body">
                                    // FILL: widget kinds — extend this match with
                                    // chart kinds (bar_chart, line_chart, …).
                                    {match kind.as_str() {
                                        "number" => view! {
                                            <div class="dashboard-tile-number">
                                                {format_value(&value)}
                                            </div>
                                        }.into_any(),
                                        "text" => view! {
                                            <div class="dashboard-tile-text">
                                                {format_value(&value)}
                                            </div>
                                        }.into_any(),
                                        // FILL: chart rendering — render bar/line
                                        // chart with the widget's history (requires
                                        // sqlite seam for time-series storage).
                                        other => view! {
                                            <div class="dashboard-tile-text">
                                                {format!("(unknown kind: {other})")}
                                            </div>
                                        }.into_any(),
                                    }}
                                </div>
                                <footer class="dashboard-tile-foot">
                                    {move || match source.as_deref() {
                                        Some(u) => view! {
                                            <span class="dashboard-tile-meta" title=u.to_string()>
                                                {short_host(u)}
                                            </span>
                                        }.into_any(),
                                        None => view! { <span></span> }.into_any(),
                                    }}
                                    {match fetched {
                                        Some(ms) => view! {
                                            <span class="dashboard-tile-meta">{format_time(ms)}</span>
                                        }.into_any(),
                                        None => view! { <span></span> }.into_any(),
                                    }}
                                </footer>
                            </article>
                        }
                    }).collect_view().into_any()
                }}
            </section>
        </div>
    }
}

fn format_value(v: &Option<Json>) -> String {
    match v {
        None => "—".into(),
        Some(Json::Null) => "—".into(),
        Some(Json::String(s)) => s.clone(),
        Some(Json::Number(n)) => n.to_string(),
        Some(Json::Bool(b)) => b.to_string(),
        Some(other) => other.to_string(),
    }
}

fn short_host(url: &str) -> String {
    url.trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or(url)
        .to_string()
}

fn format_time(ms: u64) -> String {
    if ms == 0 {
        return String::new();
    }
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms as f64));
    let s: String = date.to_iso_string().into();
    if s.len() >= 16 {
        s[..16].replace('T', " ")
    } else {
        s
    }
}
