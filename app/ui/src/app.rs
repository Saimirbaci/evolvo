use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};

use crate::canvas::{CanvasController, CanvasSurface};
use crate::feedback_panel::FeedbackPanel;
use crate::interop;
use crate::toolbar::Toolbar;
use crate::types::{AppHealth, FeedbackRecord, SandboxJobRecord};

/// GitHub URL the "Star Us" nav shortcut opens. Update this if the repo
/// moves. Kept as a constant rather than a build-time env var so the binary
/// ships with a known, auditable destination.
const STAR_REPO_URL: &str = "https://github.com/saimirbaci/NoIDE";

fn star_us_link() -> leptos::prelude::AnyView {
    view! {
        <button
            class="app-bar-link star-us-link"
            title="Star this repo on GitHub"
            aria-label="Star NoIDE on GitHub"
            on:click=move |_| {
                spawn_local(async move {
                    let _ = interop::open_external_url(STAR_REPO_URL).await;
                });
            }
        >
            <span class="star-us-icon" aria-hidden="true">"★"</span>
            "Star Us"
        </button>
    }.into_any()
}

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
                    {View::all().into_iter().flat_map(|v| {
                        let is_active = move || view_sig.get() == v;
                        let primary = view! {
                            <button
                                class="app-bar-link"
                                class:active=is_active
                                on:click=move |_| view_sig.set(v)
                            >
                                {v.label()}
                            </button>
                        }.into_any();
                        // Inject the "Star Us" shortcut just before the
                        // Sandbox tab so it sits at the left edge of the
                        // sandbox section of the nav.
                        if matches!(v, View::Sandbox) {
                            vec![star_us_link(), primary]
                        } else {
                            vec![primary]
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
    let selected: RwSignal<Option<String>> = RwSignal::new(None);
    Effect::new(move |_: Option<()>| {
        let _ = reload.get();
        spawn_local(async move {
            let result = interop::list_sandbox_jobs().await;
            items.set(Some(result));
        });
    });

    // Keep selection valid — if nothing selected but records exist, select the
    // first; if the selected id disappeared, fall back.
    Effect::new(move |_: Option<()>| {
        let Some(Ok(ref records)) = items.get() else { return };
        let current = selected.get_untracked();
        let valid = current
            .as_ref()
            .map(|id| records.iter().any(|r| &r.id == id))
            .unwrap_or(false);
        if !valid {
            selected.set(records.first().map(|r| r.id.clone()));
        }
    });

    view! {
        <div class="sandbox-page">
            <aside class="sandbox-sidebar">
                <h2 class="sandbox-sidebar-title">"Sandbox"</h2>
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
                                <ul class="sandbox-list">
                                    {records.into_iter().map(|r| {
                                        let id = r.id.clone();
                                        let id_for_click = id.clone();
                                        let is_active = move || selected.get().as_deref() == Some(&id);
                                        view! {
                                            <li>
                                                <button
                                                    class="sandbox-list-item"
                                                    class:active=is_active
                                                    on:click=move |_| selected.set(Some(id_for_click.clone()))
                                                >
                                                    <div class="sandbox-list-item-head">
                                                        <span class="sandbox-list-item-title">{r.title.clone()}</span>
                                                        <span class="list-card-status">{r.status.label()}</span>
                                                    </div>
                                                    <div class="list-card-meta">
                                                        {if r.iteration > 0 {
                                                            format!("iter {} · {}", r.iteration, format_time(r.created_at_unix_ms))
                                                        } else {
                                                            format_time(r.created_at_unix_ms)
                                                        }}
                                                    </div>
                                                </button>
                                            </li>
                                        }
                                    }).collect_view()}
                                </ul>
                            }.into_any()
                        }
                    }
                }}
            </aside>
            <section class="sandbox-detail">
                {move || {
                    let Some(Ok(records)) = items.get() else {
                        return view! { <div class="empty-state">"Loading…"</div> }.into_any();
                    };
                    let Some(id) = selected.get() else {
                        return view! {
                            <div class="empty-state">"Select a sandbox job to view activity."</div>
                        }.into_any();
                    };
                    let Some(record) = records.into_iter().find(|r| r.id == id) else {
                        return view! { <div class="empty-state">"No selection."</div> }.into_any();
                    };
                    view! { <SandboxDetail record=record reload=reload /> }.into_any()
                }}
            </section>
        </div>
    }
}

#[component]
fn SandboxDetail(record: SandboxJobRecord, reload: RwSignal<u32>) -> impl IntoView {
    let can_approve = record.status.can_approve();
    let can_retry = record.status.can_retry();
    let can_run = record.status.can_run() && record.worktree_path.is_some();
    let approve_id = record.id.clone();
    let reject_id = record.id.clone();
    let retry_id = record.id.clone();
    let run_id = record.id.clone();

    // Inline "Fix" clarification editor state.
    let fix_open: RwSignal<bool> = RwSignal::new(false);
    let fix_text: RwSignal<String> = RwSignal::new(String::new());
    let fix_submitting: RwSignal<bool> = RwSignal::new(false);

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
    let run_click = move |_| {
        let id = run_id.clone();
        spawn_local(async move {
            let _ = interop::run_sandbox_job(&id).await;
            reload.update(|v| *v = v.wrapping_add(1));
        });
        schedule_reloads(reload, &[1500, 4000, 10_000]);
    };
    let submit_fix = move || {
        if fix_submitting.get_untracked() {
            return;
        }
        let text = fix_text.get_untracked().trim().to_string();
        let id = retry_id.clone();
        fix_submitting.set(true);
        spawn_local(async move {
            if !text.is_empty() {
                let note = format!("User clarification: {text}");
                let _ = interop::append_sandbox_note(&id, &note).await;
            }
            let _ = interop::retry_sandbox_job(&id).await;
            fix_submitting.set(false);
            fix_open.set(false);
            fix_text.set(String::new());
            reload.update(|v| *v = v.wrapping_add(1));
        });
    };

    let branch = record.branch_name.clone();
    let worktree = record.worktree_path.clone();
    let log = record.log_path.clone();
    let notes = record.notes.clone();
    let iteration = record.iteration;
    let title = record.title.clone();
    let summary = record.summary.clone();
    let status_label = record.status.label();
    let created = format_time(record.created_at_unix_ms);

    view! {
        <div class="sandbox-detail-inner">
            <header class="sandbox-detail-head">
                <div>
                    <h3 class="sandbox-detail-title">{title}</h3>
                    <div class="list-card-meta">
                        {if iteration > 0 {
                            format!("iteration {iteration} · {created}")
                        } else {
                            created
                        }}
                    </div>
                </div>
                <span class="list-card-status">{status_label}</span>
            </header>

            <p class="sandbox-detail-summary">{summary}</p>

            {match branch {
                Some(b) => view! {
                    <div class="list-card-meta"><strong>"branch: "</strong>{b}</div>
                }.into_any(),
                None => view! { <span></span> }.into_any(),
            }}
            {match worktree {
                Some(w) => view! {
                    <div class="list-card-meta"><strong>"worktree: "</strong><code>{w}</code></div>
                }.into_any(),
                None => view! { <span></span> }.into_any(),
            }}
            {match log {
                Some(l) => view! {
                    <div class="list-card-meta"><strong>"log: "</strong><code>{l}</code></div>
                }.into_any(),
                None => view! { <span></span> }.into_any(),
            }}

            <section class="sandbox-activity">
                <h4>{format!("Activity ({} entries)", notes.len())}</h4>
                {if notes.is_empty() {
                    view! {
                        <div class="empty-state">"No activity yet."</div>
                    }.into_any()
                } else {
                    view! {
                        <ul class="sandbox-activity-list">
                            {notes.into_iter().map(|n| view! { <li>{n}</li> }).collect_view()}
                        </ul>
                    }.into_any()
                }}
            </section>

            {move || {
                if !fix_open.get() {
                    return view! { <span></span> }.into_any();
                }
                let submit = submit_fix.clone();
                view! {
                    <div class="sandbox-fix-form">
                        <label class="sandbox-fix-label">
                            "Describe the problem or give Claude more context for the fix:"
                        </label>
                        <textarea
                            class="sandbox-fix-textarea"
                            placeholder="e.g. build failed because the new command wasn't registered in invoke_handler…"
                            prop:value=move || fix_text.get()
                            on:input=move |ev| {
                                let t = event_target::<web_sys::HtmlTextAreaElement>(&ev);
                                fix_text.set(t.value());
                            }
                        ></textarea>
                        <div class="sandbox-fix-actions">
                            <button
                                class="secondary-btn"
                                prop:disabled=move || fix_submitting.get()
                                on:click=move |_| {
                                    fix_open.set(false);
                                    fix_text.set(String::new());
                                }
                            >
                                "Cancel"
                            </button>
                            <button
                                class="primary-btn"
                                prop:disabled=move || fix_submitting.get()
                                on:click=move |_| submit()
                            >
                                {move || if fix_submitting.get() { "Submitting…" } else { "Submit fix" }}
                            </button>
                        </div>
                    </div>
                }.into_any()
            }}

            <div class="sandbox-detail-actions">
                <button class="secondary-btn" on:click=reject_click>"Reject"</button>
                <button
                    class="secondary-btn"
                    prop:disabled=!can_retry
                    on:click=move |_| fix_open.update(|v| *v = !*v)
                    title="Send a clarification and re-run this iteration"
                >
                    "Fix"
                </button>
                <button
                    class="secondary-btn"
                    prop:disabled=!can_run
                    on:click=run_click
                    title="Launch the app built in this iteration's worktree"
                >
                    "Run"
                </button>
                <button
                    class="primary-btn"
                    prop:disabled=!can_approve
                    on:click=approve_click
                    title="Plan then implement the next iteration from this feedback"
                >
                    "Evolve"
                </button>
            </div>
        </div>
    }
}

fn format_time(ms: u64) -> String {
    let date = js_sys::Date::new(&JsValue::from_f64(ms as f64));
    String::from(date.to_iso_string())
}

/// Bump `reload` after each delay (ms). Used to surface asynchronous sandbox
/// notes (e.g. the Run button's background-thread spawn result) without
/// requiring the user to navigate away and back.
fn schedule_reloads(reload: RwSignal<u32>, delays_ms: &[i32]) {
    let Some(win) = web_sys::window() else {
        return;
    };
    for &ms in delays_ms {
        let r = reload;
        let cb = Closure::once_into_js(move || {
            r.update(|v| *v = v.wrapping_add(1));
        });
        let _ = win.set_timeout_with_callback_and_timeout_and_arguments_0(
            cb.unchecked_ref(),
            ms,
        );
    }
}
