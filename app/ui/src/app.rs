use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};

use crate::canvas::{CanvasController, CanvasSurface};
use crate::feedback_panel::FeedbackPanel;
use crate::interop;
use crate::toolbar::Toolbar;
use crate::types::{AppHealth, SandboxJobRecord};

/// GitHub URL the "Star Us" nav shortcut opens. Update this if the repo
/// moves. Kept as a constant rather than a build-time env var so the binary
/// ships with a known, auditable destination.
const STAR_REPO_URL: &str = "https://github.com/saimirbaci/Evolvo";

fn star_us_link() -> leptos::prelude::AnyView {
    view! {
        <button
            class="app-bar-link star-us-link"
            title="Star this repo on GitHub"
            aria-label="Star Evolvo on GitHub"
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
    Lineage,
}

impl View {
    fn route(self) -> &'static str {
        match self {
            Self::Home => "/",
            Self::Lineage => "/lineage",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Home => "Canvas",
            Self::Lineage => "Lineage",
        }
    }

    fn all() -> [View; 2] {
        [Self::Home, Self::Lineage]
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

    let panel_open: RwSignal<bool> = RwSignal::new(false);
    let ctrl_fab = controller.clone();
    let ctrl_toolbar = controller.clone();
    let ctrl_canvas = controller.clone();
    let ctrl_panel = controller.clone();

    view! {
        <div class="app-root">
            <header class="app-bar">
                <div>
                    <span class="app-bar-title">"Evolvo"</span>
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
                        }.into_any()
                    }).collect_view()}
                    {star_us_link()}
                </nav>
            </header>

            {move || match view_sig.get() {
                View::Home => view! { <HomePage panel_open=panel_open /> }.into_any(),
                View::Lineage => view! { <SandboxPage /> }.into_any(),
            }}

            {move || {
                if panel_open.get() {
                    view! {
                        <div class="canvas-overlay">
                            <Toolbar controller=ctrl_toolbar.clone() />
                            <CanvasSurface controller=ctrl_canvas.clone() />
                        </div>
                        <FeedbackPanel
                            controller=ctrl_panel.clone()
                            route=route
                            is_open=panel_open
                        />
                    }.into_any()
                } else {
                    view! { <span></span> }.into_any()
                }
            }}

            <FeedbackFab controller=ctrl_fab.clone() is_open=panel_open />
        </div>
    }
}

#[component]
fn HomePage(panel_open: RwSignal<bool>) -> impl IntoView {
    // Hide the welcome copy while the canvas overlay is open so it doesn't
    // end up in the page screenshot that ships with the feedback submission.
    view! {
        <div class="home-page" class:hidden=move || panel_open.get()>
            <div class="home-hero">
                <h1 class="home-title">"Welcome to Evolvo"</h1>
                <p class="home-subtitle">
                    "Click the ✎ button in the bottom-right corner of any page \
                     to open the Canvas overlay and send feedback about what \
                     you're looking at."
                </p>
                <ul class="home-tips">
                    <li>"Draw, type, or paste screenshots directly on the page."</li>
                    <li>"Record a voice note to add context."</li>
                    <li>"Submit to kick off a lineage iteration."</li>
                </ul>
            </div>
        </div>
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
        <div class="lineage-page">
            <aside class="lineage-sidebar">
                <h2 class="lineage-sidebar-title">"Lineage"</h2>
                {move || match items.get() {
                    None => view! { <div class="empty-state">"Loading…"</div> }.into_any(),
                    Some(Err(e)) => view! {
                        <div class="empty-state">{format!("Failed to load: {e}")}</div>
                    }.into_any(),
                    Some(Ok(records)) => {
                        if records.is_empty() {
                            view! {
                                <div class="empty-state">"No lineage jobs yet."</div>
                            }.into_any()
                        } else {
                            view! {
                                <ul class="lineage-list">
                                    {records.into_iter().map(|r| {
                                        let id = r.id.clone();
                                        let id_for_click = id.clone();
                                        let is_active = move || selected.get().as_deref() == Some(&id);
                                        view! {
                                            <li>
                                                <button
                                                    class="lineage-list-item"
                                                    class:active=is_active
                                                    on:click=move |_| selected.set(Some(id_for_click.clone()))
                                                >
                                                    <div class="lineage-list-item-head">
                                                        <span class="lineage-list-item-title">{r.title.clone()}</span>
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
            <section class="lineage-detail">
                {move || {
                    let Some(Ok(records)) = items.get() else {
                        return view! { <div class="empty-state">"Loading…"</div> }.into_any();
                    };
                    let Some(id) = selected.get() else {
                        return view! {
                            <div class="empty-state">"Select a lineage job to view activity."</div>
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
        <div class="lineage-detail-inner">
            <header class="lineage-detail-head">
                <div>
                    <h3 class="lineage-detail-title">{title}</h3>
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

            <p class="lineage-detail-summary">{summary}</p>

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

            <section class="lineage-activity">
                <h4>{format!("Activity ({} entries)", notes.len())}</h4>
                {if notes.is_empty() {
                    view! {
                        <div class="empty-state">"No activity yet."</div>
                    }.into_any()
                } else {
                    view! {
                        <ul class="lineage-activity-list">
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
                    <div class="lineage-fix-form">
                        <label class="lineage-fix-label">
                            "Describe the problem or give Claude more context for the fix:"
                        </label>
                        <textarea
                            class="lineage-fix-textarea"
                            placeholder="e.g. build failed because the new command wasn't registered in invoke_handler…"
                            prop:value=move || fix_text.get()
                            on:input=move |ev| {
                                let t = event_target::<web_sys::HtmlTextAreaElement>(&ev);
                                fix_text.set(t.value());
                            }
                        ></textarea>
                        <div class="lineage-fix-actions">
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

            <div class="lineage-detail-actions">
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

/// Bump `reload` after each delay (ms). Used to surface asynchronous lineage
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
