use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsValue;

use crate::canvas::{CanvasController, CanvasSurface};
use crate::feedback_panel::FeedbackPanel;
use crate::interop;
use crate::toolbar::Toolbar;
use crate::types::{AppHealth, FeedbackRecord, SandboxJobRecord};

#[derive(Copy, Clone, PartialEq, Eq, Default)]
pub enum View {
    #[default]
    Home,
    Inbox,
    Sandbox,
}

impl View {
    fn route(self) -> &'static str {
        match self {
            Self::Home => "/",
            Self::Inbox => "/inbox",
            Self::Sandbox => "/sandbox",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Home => "Canvas",
            Self::Inbox => "Inbox",
            Self::Sandbox => "Sandbox",
        }
    }

    fn all() -> [View; 3] {
        [Self::Home, Self::Inbox, Self::Sandbox]
    }
}

#[component]
pub fn App() -> impl IntoView {
    let view_sig: RwSignal<View> = RwSignal::new(View::Home);
    let route: RwSignal<String> = RwSignal::new(View::Home.route().to_string());
    let controller = CanvasController::new();
    let health: RwSignal<Option<AppHealth>> = RwSignal::new(None);

    Effect::new(move |already: Option<()>| {
        if already.is_some() {
            return;
        }
        spawn_local(async move {
            if let Ok(h) = interop::app_health().await {
                health.set(Some(h));
            }
        });
    });

    Effect::new(move |_| {
        route.set(view_sig.get().route().to_string());
    });

    let controller_for_home = controller.clone();

    view! {
        <div class="app-root">
            <header class="app-bar">
                <div>
                    <span class="app-bar-title">"NoIDE"</span>
                    <span class="app-bar-subtitle">
                        {move || match health.get() {
                            Some(h) => format!("v{} • {}", h.app_version, h.workspace_path),
                            None => "loading…".into(),
                        }}
                    </span>
                </div>
                <nav class="app-bar-actions">
                    {View::all().into_iter().map(|v| {
                        let is_active = move || view_sig.get() == v;
                        view! {
                            <button
                                class="app-bar-link"
                                class:active=is_active
                                on:click=move |_| view_sig.set(v)
                            >
                                {v.label()}
                            </button>
                        }
                    }).collect_view()}
                </nav>
            </header>

            {move || match view_sig.get() {
                View::Home => view! {
                    <HomePage controller=controller_for_home.clone() route=route />
                }.into_any(),
                View::Inbox => view! { <InboxPage /> }.into_any(),
                View::Sandbox => view! { <SandboxPage /> }.into_any(),
            }}
        </div>
    }
}

#[component]
fn HomePage(controller: CanvasController, route: RwSignal<String>) -> impl IntoView {
    let ctrl_canvas = controller.clone();
    let ctrl_toolbar = controller.clone();
    let ctrl_fab = controller.clone();
    let panel_open: RwSignal<bool> = RwSignal::new(false);

    view! {
        <Toolbar controller=ctrl_toolbar />
        <CanvasSurface controller=ctrl_canvas />

        {move || {
            if panel_open.get() {
                view! {
                    <FeedbackPanel
                        controller=controller.clone()
                        route=route
                        is_open=panel_open
                    />
                }
                .into_any()
            } else {
                view! { <span></span> }.into_any()
            }
        }}

        <FeedbackFab controller=ctrl_fab is_open=panel_open />
    }
}

#[component]
fn FeedbackFab(controller: CanvasController, is_open: RwSignal<bool>) -> impl IntoView {
    view! {
        <button
            class="fab"
            class:fab-active=move || is_open.get()
            title="Send feedback"
            aria-label="Send feedback"
            on:click=move |_| is_open.update(|v| *v = !*v)
        >
            {move || {
                if is_open.get() {
                    "×"
                } else {
                    "✎"
                }
            }}
            {move || {
                let pending = controller.pasted_base64.get().len() + controller.shapes.get().len();
                if pending > 0 && !is_open.get() {
                    view! { <span class="fab-badge">{pending}</span> }.into_any()
                } else {
                    view! { <span></span> }.into_any()
                }
            }}
        </button>
    }
}

#[component]
fn InboxPage() -> impl IntoView {
    let items: RwSignal<Option<Result<Vec<FeedbackRecord>, String>>> = RwSignal::new(None);
    Effect::new(move |already: Option<()>| {
        if already.is_some() {
            return;
        }
        spawn_local(async move {
            let result = interop::list_feedback().await;
            items.set(Some(result));
        });
    });
    view! {
        <div class="list-page">
            <h2>"Feedback inbox"</h2>
            {move || match items.get() {
                None => view! { <div class="empty-state">"Loading…"</div> }.into_any(),
                Some(Err(e)) => view! {
                    <div class="empty-state">{format!("Failed to load: {e}")}</div>
                }.into_any(),
                Some(Ok(records)) => {
                    if records.is_empty() {
                        view! {
                            <div class="empty-state">
                                "No feedback yet — go draw on the canvas and submit something."
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="list-grid">
                                {records.into_iter().map(|r| view! { <FeedbackCard record=r /> }).collect_view()}
                            </div>
                        }.into_any()
                    }
                }
            }}
        </div>
    }
}

#[component]
fn FeedbackCard(record: FeedbackRecord) -> impl IntoView {
    let title = record
        .feedback_text
        .lines()
        .next()
        .unwrap_or("(no text)")
        .to_string();
    let extras = format!(
        "{} annotation(s) • {} image(s){}",
        record.annotations.len(),
        record.pasted_images.len(),
        if record.voice_filename.is_some() {
            " • voice"
        } else {
            ""
        },
    );
    view! {
        <div class="list-card">
            <div class="list-card-head">
                <span class="list-card-title">{record.feedback_type.label()}</span>
                <span class="list-card-status">{record.status.label()}</span>
            </div>
            <div class="list-card-meta">{format_time(record.created_at_unix_ms)}</div>
            <div class="list-card-body">{title}</div>
            <div class="list-card-meta">{extras}</div>
        </div>
    }
}

#[component]
fn SandboxPage() -> impl IntoView {
    let items: RwSignal<Option<Result<Vec<SandboxJobRecord>, String>>> = RwSignal::new(None);
    let reload = RwSignal::new(0_u32);
    Effect::new(move |_: Option<()>| {
        let _ = reload.get();
        spawn_local(async move {
            let result = interop::list_sandbox_jobs().await;
            items.set(Some(result));
        });
    });
    view! {
        <div class="list-page">
            <h2>"Sandbox"</h2>
            {move || match items.get() {
                None => view! { <div class="empty-state">"Loading…"</div> }.into_any(),
                Some(Err(e)) => view! {
                    <div class="empty-state">{format!("Failed to load: {e}")}</div>
                }.into_any(),
                Some(Ok(records)) => {
                    if records.is_empty() {
                        view! {
                            <div class="empty-state">"No sandbox jobs yet."</div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="list-grid">
                                {records.into_iter().map(|r| view! {
                                    <SandboxCard record=r reload=reload />
                                }).collect_view()}
                            </div>
                        }.into_any()
                    }
                }
            }}
        </div>
    }
}

#[component]
fn SandboxCard(record: SandboxJobRecord, reload: RwSignal<u32>) -> impl IntoView {
    let can_approve = record.status.can_approve();
    let approve_id = record.id.clone();
    let reject_id = record.id.clone();
    let approve_click = move |_| {
        let id = approve_id.clone();
        spawn_local(async move {
            let _ = interop::approve_sandbox_job(&id).await;
            reload.update(|v| *v = v.wrapping_add(1));
        });
    };
    let reject_click = move |_| {
        let id = reject_id.clone();
        spawn_local(async move {
            let _ = interop::reject_sandbox_job(&id).await;
            reload.update(|v| *v = v.wrapping_add(1));
        });
    };
    view! {
        <div class="list-card">
            <div class="list-card-head">
                <span class="list-card-title">{record.title.clone()}</span>
                <span class="list-card-status">{record.status.label()}</span>
            </div>
            <div class="list-card-meta">{format_time(record.created_at_unix_ms)}</div>
            <div class="list-card-body">{record.summary.clone()}</div>
            <div class="list-card-actions">
                <button class="secondary-btn" on:click=reject_click>"Reject"</button>
                <button
                    class="primary-btn"
                    prop:disabled=!can_approve
                    on:click=approve_click
                >
                    "Advance"
                </button>
            </div>
        </div>
    }
}

fn format_time(ms: u64) -> String {
    let date = js_sys::Date::new(&JsValue::from_f64(ms as f64));
    String::from(date.to_iso_string())
}
