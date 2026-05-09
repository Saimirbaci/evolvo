//! survey_form — UI scaffold (the NewApp content area).
//!
//! Drop into `app/ui/src/app_shape_survey_form.rs`, declare the new
//! module in `app/ui/src/main.rs`, and rewrite `app/ui/src/app.rs` so
//! its `App` component wraps `<SurveyFormPage/>` inside `<Shell>`.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{json, Map as JsonMap, Value as Json};
use std::collections::HashMap;
use wasm_bindgen::JsCast;

use crate::interop;
use crate::types::Response;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    TextArea,
    Number,
    Date,
    /// Checkbox returning a bool.
    Checkbox,
    /// Single-choice radio with the given options.
    Radio(&'static [&'static str]),
}

#[derive(Clone, Debug)]
pub struct FieldDef {
    pub id: &'static str,
    pub label: &'static str,
    pub kind: FieldKind,
    pub required: bool,
}

fn fields() -> Vec<FieldDef> {
    // FILL: form schema — declare every field the form should render.
    // Each field's `id` becomes the JSON key in `Response.values`.
    vec![
        FieldDef {
            id: "name",
            label: "Your name",
            kind: FieldKind::Text,
            required: true,
        },
        FieldDef {
            id: "feedback",
            label: "What can we improve?",
            kind: FieldKind::TextArea,
            required: false,
        },
        FieldDef {
            id: "rating",
            label: "Rating (1-5)",
            kind: FieldKind::Number,
            required: false,
        },
        FieldDef {
            id: "subscribe",
            label: "Send me product updates",
            kind: FieldKind::Checkbox,
            required: false,
        },
    ]
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Fill,
    Review,
}

#[component]
pub fn SurveyFormPage() -> impl IntoView {
    let tab: RwSignal<Tab> = RwSignal::new(Tab::Fill);
    let values: RwSignal<HashMap<String, Json>> = RwSignal::new(HashMap::new());
    let responses: RwSignal<Vec<Response>> = RwSignal::new(Vec::new());
    let load_error: RwSignal<Option<String>> = RwSignal::new(None);
    let submitting: RwSignal<bool> = RwSignal::new(false);
    let success_at: RwSignal<Option<u64>> = RwSignal::new(None);

    let reload: RwSignal<u32> = RwSignal::new(0);
    Effect::new(move |_| {
        let _ = reload.get();
        spawn_local(async move {
            match interop::list_responses().await {
                Ok(rs) => responses.set(rs),
                Err(e) => load_error.set(Some(e)),
            }
        });
    });

    let submit = move || {
        if submitting.get_untracked() {
            return;
        }
        // FILL: validation rules — for each `field.required`, refuse to
        // submit when the value is missing or empty. Surface a per-field
        // error rather than a single banner.
        let mut payload = JsonMap::new();
        let snapshot = values.get_untracked();
        for f in fields() {
            let v = snapshot.get(f.id).cloned().unwrap_or(Json::Null);
            payload.insert(f.id.to_string(), v);
        }
        let json_payload = Json::Object(payload);
        submitting.set(true);
        spawn_local(async move {
            match interop::save_response(json_payload).await {
                Ok(r) => {
                    success_at.set(Some(r.submitted_at));
                    values.set(HashMap::new());
                    reload.update(|v| *v = v.wrapping_add(1));
                }
                Err(e) => load_error.set(Some(e)),
            }
            submitting.set(false);
        });
    };

    // FILL: keyboard shortcuts — ⌘⏎ to submit.

    view! {
        <div class="survey-page">
            <header class="survey-tabs" role="tablist">
                <button
                    class="survey-tab"
                    class:active=move || tab.get() == Tab::Fill
                    on:click=move |_| tab.set(Tab::Fill)
                    role="tab"
                >
                    "Fill"
                </button>
                <button
                    class="survey-tab"
                    class:active=move || tab.get() == Tab::Review
                    on:click=move |_| tab.set(Tab::Review)
                    role="tab"
                >
                    {move || format!("Review ({})", responses.get().len())}
                </button>
            </header>

            {move || match load_error.get() {
                Some(e) => view! {
                    <div class="survey-error" role="alert">{e}</div>
                }.into_any(),
                None => view! { <span></span> }.into_any(),
            }}

            {move || match tab.get() {
                Tab::Fill => view! {
                    <FillForm values=values submitting=submitting success_at=success_at submit=submit.clone() />
                }.into_any(),
                Tab::Review => view! {
                    <ReviewList responses=responses />
                }.into_any(),
            }}
        </div>
    }
}

#[component]
fn FillForm<F>(
    values: RwSignal<HashMap<String, Json>>,
    submitting: RwSignal<bool>,
    success_at: RwSignal<Option<u64>>,
    submit: F,
) -> impl IntoView
where
    F: Fn() + Clone + 'static,
{
    let submit_handler = submit.clone();
    view! {
        <form
            class="survey-form"
            on:submit=move |ev| {
                ev.prevent_default();
                submit_handler.clone()();
            }
        >
            {fields().into_iter().map(|f| render_field(f, values)).collect_view()}
            <div class="survey-actions">
                <button
                    class="primary-btn"
                    type="submit"
                    prop:disabled=move || submitting.get()
                    aria-label="Submit survey"
                >
                    {move || if submitting.get() { "Submitting…" } else { "Submit" }}
                </button>
            </div>
            {move || {
                if success_at.get().is_some() {
                    view! {
                        <div class="survey-success" role="status">
                            // FILL: thank-you copy — replace with the message
                            // appropriate for this form's audience.
                            "Thanks — your response was recorded."
                        </div>
                    }.into_any()
                } else {
                    view! { <span></span> }.into_any()
                }
            }}
        </form>
    }
}

fn render_field(f: FieldDef, values: RwSignal<HashMap<String, Json>>) -> AnyView {
    let id = f.id.to_string();
    let label = f.label.to_string();
    let label_text = if f.required {
        format!("{label} *")
    } else {
        label
    };
    let id_for_change = id.clone();
    let id_for_value = id.clone();
    match f.kind {
        FieldKind::Text => view! {
            <label class="survey-field">
                <span class="survey-field-label">{label_text}</span>
                <input
                    class="survey-input"
                    type="text"
                    on:input=move |ev| {
                        let t = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                        if let Some(t) = t {
                            let mut current = values.get_untracked();
                            current.insert(id_for_change.clone(), Json::String(t.value()));
                            values.set(current);
                        }
                    }
                    prop:value=move || values.get().get(&id_for_value).and_then(|v| v.as_str().map(|s| s.to_string())).unwrap_or_default()
                />
            </label>
        }
        .into_any(),
        FieldKind::TextArea => view! {
            <label class="survey-field">
                <span class="survey-field-label">{label_text}</span>
                <textarea
                    class="survey-textarea"
                    on:input=move |ev| {
                        let t = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlTextAreaElement>().ok());
                        if let Some(t) = t {
                            let mut current = values.get_untracked();
                            current.insert(id_for_change.clone(), Json::String(t.value()));
                            values.set(current);
                        }
                    }
                    prop:value=move || values.get().get(&id_for_value).and_then(|v| v.as_str().map(|s| s.to_string())).unwrap_or_default()
                ></textarea>
            </label>
        }
        .into_any(),
        FieldKind::Number => view! {
            <label class="survey-field">
                <span class="survey-field-label">{label_text}</span>
                <input
                    class="survey-input"
                    type="number"
                    on:input=move |ev| {
                        let t = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                        if let Some(t) = t {
                            let mut current = values.get_untracked();
                            let parsed: Option<f64> = t.value().parse().ok();
                            current.insert(
                                id_for_change.clone(),
                                parsed.map(|n| json!(n)).unwrap_or(Json::Null),
                            );
                            values.set(current);
                        }
                    }
                />
            </label>
        }
        .into_any(),
        FieldKind::Date => view! {
            <label class="survey-field">
                <span class="survey-field-label">{label_text}</span>
                <input
                    class="survey-input"
                    type="date"
                    on:change=move |ev| {
                        let t = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                        if let Some(t) = t {
                            let mut current = values.get_untracked();
                            current.insert(id_for_change.clone(), Json::String(t.value()));
                            values.set(current);
                        }
                    }
                />
            </label>
        }
        .into_any(),
        FieldKind::Checkbox => view! {
            <label class="survey-field survey-field-checkbox">
                <input
                    class="survey-checkbox"
                    type="checkbox"
                    on:change=move |ev| {
                        let t = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                        if let Some(t) = t {
                            let mut current = values.get_untracked();
                            current.insert(id_for_change.clone(), Json::Bool(t.checked()));
                            values.set(current);
                        }
                    }
                />
                <span class="survey-field-label">{label_text}</span>
            </label>
        }
        .into_any(),
        FieldKind::Radio(opts) => view! {
            <fieldset class="survey-field survey-field-radio">
                <legend class="survey-field-label">{label_text}</legend>
                {opts.iter().map(|opt| {
                    let id_for_change2 = id_for_change.clone();
                    let opt_value = opt.to_string();
                    let opt_value_clone = opt_value.clone();
                    let id_for_active = id_for_value.clone();
                    let opt_for_active = opt_value.clone();
                    view! {
                        <label class="survey-radio-option">
                            <input
                                type="radio"
                                name=id_for_change.clone()
                                value=opt_value.clone()
                                prop:checked=move || values.get().get(&id_for_active).and_then(|v| v.as_str()) == Some(opt_for_active.as_str())
                                on:change=move |_| {
                                    let mut current = values.get_untracked();
                                    current.insert(id_for_change2.clone(), Json::String(opt_value_clone.clone()));
                                    values.set(current);
                                }
                            />
                            <span>{opt.to_string()}</span>
                        </label>
                    }
                }).collect_view()}
            </fieldset>
        }
        .into_any(),
    }
}

#[component]
fn ReviewList(responses: RwSignal<Vec<Response>>) -> impl IntoView {
    view! {
        <section class="survey-review">
            {move || {
                let rs = responses.get();
                if rs.is_empty() {
                    return view! {
                        <div class="empty-state">"No responses yet."</div>
                    }.into_any();
                }
                view! {
                    <ul class="survey-review-list">
                        {rs.into_iter().map(|r| view! {
                            <li class="survey-review-item">
                                <header>
                                    <span>{format_time(r.submitted_at)}</span>
                                    <code>{r.id.clone()}</code>
                                </header>
                                <pre class="survey-review-values">
                                    {serde_json::to_string_pretty(&r.values).unwrap_or_default()}
                                </pre>
                            </li>
                        }).collect_view()}
                    </ul>
                }.into_any()
            }}
        </section>
    }
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
