//! Invariant application shell.
//!
//! `Shell` is the permanent chrome wrapped around whatever app the current
//! iteration is building. It owns the four load-bearing surfaces that every
//! iteration must preserve:
//!
//! - the app bar with the Lineage navigation + "Star Us" link,
//! - the single Feedback FAB that opens the Canvas overlay **and** the
//!   feedback submission panel together (one trigger, one signal, both
//!   surfaces — see I-P3b in `.claude/rules/common/product-invariants.md`),
//! - the Canvas overlay + Toolbar, mountable on top of any page,
//! - the Lineage review page.
//!
//! The concrete app being built on top of Evolvo lives in `app.rs` and is
//! passed in as `children` — those children render on the Home route inside
//! the shell. `app.rs` can be rewritten freely by each iteration; `shell.rs`
//! is invariant.
//!
//! The shell exposes `panel_open` via context so children can react to the
//! Canvas overlay being open (e.g. to hide copy from a page screenshot).

use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};

use crate::canvas::{CanvasController, CanvasSurface};
use crate::feedback_panel::FeedbackPanel;
use crate::interop;
use crate::toolbar::Toolbar;
use crate::types::{AppHealth, LineageJobRecord};

/// GitHub URL the "Star Us" nav shortcut opens. Update this if the repo
/// moves. Kept as a constant rather than a build-time env var so the binary
/// ships with a known, auditable destination.
const STAR_REPO_URL: &str = "https://github.com/saimirbaci/Evolvo";

/// Context exposed by `Shell` so children (the NewApp content) can observe
/// whether the Canvas overlay / Feedback panel is currently open.
#[derive(Copy, Clone)]
pub struct PanelOpen(pub RwSignal<bool>);

fn star_us_link() -> AnyView {
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
    }
    .into_any()
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
pub fn Shell(children: ChildrenFn) -> impl IntoView {
    let view_sig: RwSignal<View> = RwSignal::new(View::Home);
    let route: RwSignal<String> = RwSignal::new(View::Home.route().to_string());
    let controller = CanvasController::new();
    let health: RwSignal<Option<AppHealth>> = RwSignal::new(None);
    let panel_open: RwSignal<bool> = RwSignal::new(false);

    provide_context(PanelOpen(panel_open));

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
                View::Home => children().into_any(),
                View::Lineage => view! { <LineagePage /> }.into_any(),
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
fn LineagePage() -> impl IntoView {
    let items: RwSignal<Option<Result<Vec<LineageJobRecord>, String>>> = RwSignal::new(None);
    let reload = RwSignal::new(0_u32);
    let selected: RwSignal<Option<String>> = RwSignal::new(None);
    // Sort order for the sidebar list. `true` = newest first (default), which
    // matches the user's mental model — recent work sits at the top.
    let newest_first: RwSignal<bool> = RwSignal::new(true);
    // Error surface for action buttons. Lives at the page level (not inside
    // `LineageDetail`) so that when `items` reloads and the detail component
    // is torn down + rebuilt, the error message persists long enough for the
    // user to read it.
    let action_error: RwSignal<Option<String>> = RwSignal::new(None);
    Effect::new(move |_: Option<()>| {
        let _ = reload.get();
        spawn_local(async move {
            let result = interop::list_lineage_jobs().await;
            items.set(Some(result));
        });
    });

    // Poll the backend every 2s so that (a) status changes driven by the
    // background claude run surface without a user action and (b) edits made
    // directly to the lineage_jobs JSON on disk are picked up without an app
    // restart. The backend reads from disk on each call, so a simple reload
    // bump is sufficient.
    spawn_interval(reload, 2000);

    // Keep selection valid — if nothing selected but records exist, select the
    // first; if the selected id disappeared, fall back.
    Effect::new(move |_: Option<()>| {
        let Some(Ok(ref records)) = items.get() else {
            return;
        };
        let current = selected.get_untracked();
        let valid = current
            .as_ref()
            .map(|id| records.iter().any(|r| &r.id == id))
            .unwrap_or(false);
        if !valid {
            selected.set(records.first().map(|r| r.id.clone()));
        }
    });

    // Clear any lingering action error when the user switches to a different
    // lineage job — the error was about the previous selection.
    Effect::new(move |prev: Option<Option<String>>| {
        let now = selected.get();
        if let Some(p) = prev.as_ref() {
            if p != &now {
                action_error.set(None);
            }
        }
        now
    });

    let import_click = move |_| {
        let Some(win) = web_sys::window() else { return };
        let Ok(Some(bundle_path)) = win.prompt_with_message(
            "Path to .evolvo-bundle file:",
        ) else {
            return;
        };
        if bundle_path.trim().is_empty() {
            return;
        }
        let Ok(Some(target_root)) = win.prompt_with_message(
            "Target workspace root for the new app (must be empty):",
        ) else {
            return;
        };
        if target_root.trim().is_empty() {
            return;
        }
        action_error.set(None);
        spawn_local(async move {
            match interop::import_lineage_bundle(bundle_path.trim(), target_root.trim()).await {
                Ok(s) => {
                    let msg = format!(
                        "Imported lineage {} into {} ({} feedback, {} attachments). \
                         Launch the new app with EVOLVO_WORKSPACE_ROOT={}",
                        s.primary_job_id,
                        s.workspace_root,
                        s.feedback_count,
                        s.attachment_count,
                        s.workspace_root,
                    );
                    action_error.set(Some(msg));
                }
                Err(e) => action_error.set(Some(format!("Import failed: {e}"))),
            }
        });
    };

    view! {
        <div class="lineage-page">
            <aside class="lineage-sidebar">
                <div class="lineage-sidebar-head">
                    <h2 class="lineage-sidebar-title">"Lineage"</h2>
                    <button
                        class="lineage-sort-toggle"
                        title="Open a .evolvo-bundle into a fresh workspace (I-P4)"
                        on:click=import_click
                    >
                        "Open bundle…"
                    </button>
                    <button
                        class="lineage-sort-toggle"
                        title="Toggle sort order by creation time"
                        on:click=move |_| newest_first.update(|v| *v = !*v)
                    >
                        {move || if newest_first.get() { "Newest ↓" } else { "Oldest ↑" }}
                    </button>
                </div>
                {move || match items.get() {
                    None => view! { <div class="empty-state">"Loading…"</div> }.into_any(),
                    Some(Err(e)) => view! {
                        <div class="empty-state">{format!("Failed to load: {e}")}</div>
                    }.into_any(),
                    Some(Ok(mut records)) => {
                        if records.is_empty() {
                            view! {
                                <div class="empty-state">"No lineage jobs yet."</div>
                            }.into_any()
                        } else {
                            if newest_first.get() {
                                records.sort_by(|a, b| b.created_at_unix_ms.cmp(&a.created_at_unix_ms));
                            } else {
                                records.sort_by(|a, b| a.created_at_unix_ms.cmp(&b.created_at_unix_ms));
                            }
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
                                                    <span class="lineage-list-item-title" title=r.title.clone()>{r.title.clone()}</span>
                                                    <div class="list-card-meta">
                                                        {if r.iteration > 0 {
                                                            format!("iter {} · {}", r.iteration, format_time(r.created_at_unix_ms))
                                                        } else {
                                                            format_time(r.created_at_unix_ms)
                                                        }}
                                                    </div>
                                                    <span class="list-card-status">{r.status.label()}</span>
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
                    view! { <LineageDetail record=record reload=reload action_error=action_error /> }.into_any()
                }}
            </section>
        </div>
    }
}

#[component]
fn LineageDetail(
    record: LineageJobRecord,
    reload: RwSignal<u32>,
    // Surface for backend errors from Evolve / Reject / Run / Fix. Owned by
    // `LineagePage` so the message outlives this component's reconstruction
    // when `items` reloads right after an action fires.
    action_error: RwSignal<Option<String>>,
) -> impl IntoView {
    let can_approve = record.status.can_approve();
    let can_retry = record.status.can_retry();
    let can_run = record.status.can_run() && record.worktree_path.is_some();
    // Resume is offered when a multi-stage pipeline has progress on disk
    // (some stages are tracked) AND at least one of them didn't finish
    // green. Needs a worktree to pick up where Claude left off.
    let can_resume = record.worktree_path.is_some()
        && !record.stages.is_empty()
        && record.stages.iter().any(|s| {
            !matches!(
                s.status,
                crate::types::StageStatus::Green | crate::types::StageStatus::Skipped
            )
        });
    let approve_id = record.id.clone();
    let reject_id = record.id.clone();
    let retry_id = record.id.clone();
    let run_id = record.id.clone();
    let resume_id = record.id.clone();
    let fork_id = record.id.clone();

    // Inline "Fix" clarification editor state.
    let fix_open: RwSignal<bool> = RwSignal::new(false);
    let fix_text: RwSignal<String> = RwSignal::new(String::new());
    let fix_submitting: RwSignal<bool> = RwSignal::new(false);

    let approve_click = move |_| {
        let id = approve_id.clone();
        action_error.set(None);
        spawn_local(async move {
            if let Err(e) = interop::approve_lineage_job(&id).await {
                action_error.set(Some(e));
            }
            reload.update(|v| *v = v.wrapping_add(1));
        });
    };
    let reject_click = move |_| {
        let id = reject_id.clone();
        action_error.set(None);
        spawn_local(async move {
            if let Err(e) = interop::reject_lineage_job(&id).await {
                action_error.set(Some(e));
            }
            reload.update(|v| *v = v.wrapping_add(1));
        });
    };
    let run_click = move |_| {
        let id = run_id.clone();
        action_error.set(None);
        spawn_local(async move {
            if let Err(e) = interop::run_lineage_job(&id).await {
                action_error.set(Some(e));
            }
            reload.update(|v| *v = v.wrapping_add(1));
        });
        schedule_reloads(reload, &[1500, 4000, 10_000]);
    };
    let fork_click = move |_| {
        // Operationalises product invariant I-P4 — bundle this lineage and
        // surface the resulting `.evolvo-bundle` path so the user can carry
        // it to a fresh workspace. We prompt for the destination dir so the
        // user can route bundles wherever they keep their forks; an empty
        // response defaults to `<workspace>/exports/`.
        let id = fork_id.clone();
        let win = web_sys::window();
        let dest = win
            .as_ref()
            .and_then(|w| {
                w.prompt_with_message_and_default(
                    "Destination directory for the .evolvo-bundle (blank = workspace exports/):",
                    "",
                )
                .ok()
                .flatten()
            })
            .unwrap_or_default();
        let dest_opt = if dest.trim().is_empty() {
            None
        } else {
            Some(dest.trim().to_string())
        };
        action_error.set(None);
        spawn_local(async move {
            match interop::export_lineage(&id, dest_opt.as_deref()).await {
                Ok(r) => action_error.set(Some(format!(
                    "Forked → {}. To launch as its own app: \
                     EVOLVO_WORKSPACE_ROOT=<new_dir> cargo tauri dev (after \
                     importing the bundle there).",
                    r.bundle_path
                ))),
                Err(e) => action_error.set(Some(format!("Fork failed: {e}"))),
            }
        });
    };
    let resume_click = move |_| {
        let id = resume_id.clone();
        action_error.set(None);
        spawn_local(async move {
            if let Err(e) = interop::resume_lineage_job(&id).await {
                action_error.set(Some(e));
            }
            reload.update(|v| *v = v.wrapping_add(1));
        });
        // The pipeline runs on a background thread; nudge the UI to pick up
        // stage transitions as Claude advances.
        schedule_reloads(reload, &[2000, 6000, 15_000, 30_000]);
    };
    let submit_fix = move || {
        if fix_submitting.get_untracked() {
            return;
        }
        let text = fix_text.get_untracked().trim().to_string();
        let id = retry_id.clone();
        fix_submitting.set(true);
        action_error.set(None);
        spawn_local(async move {
            if !text.is_empty() {
                let note = format!("User clarification: {text}");
                if let Err(e) = interop::append_lineage_note(&id, &note).await {
                    action_error.set(Some(e));
                }
            }
            if let Err(e) = interop::retry_lineage_job(&id).await {
                action_error.set(Some(e));
            }
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
    let stages = record.stages.clone();

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

            {if stages.is_empty() {
                view! { <span></span> }.into_any()
            } else {
                let total = stages.len();
                let green = stages.iter().filter(|s| matches!(s.status, crate::types::StageStatus::Green)).count();
                view! {
                    <section class="lineage-stages">
                        <h4>{format!("Pipeline ({green}/{total} green)")}</h4>
                        <ul class="stage-list">
                            {stages.into_iter().map(|s| {
                                let icon = s.status.icon();
                                let kind = s.kind.label();
                                let status = s.status.label();
                                let headline = s.headline.clone().unwrap_or_default();
                                let elapsed = match (s.started_at_unix_ms, s.finished_at_unix_ms) {
                                    (Some(a), Some(b)) if b >= a => format!("{}s", (b - a) / 1000),
                                    (Some(_), None) => "running…".to_string(),
                                    _ => String::new(),
                                };
                                let status_class = format!("stage-status stage-status-{}", s.status.label());
                                view! {
                                    <li class="stage-row">
                                        <span class="stage-icon">{icon}</span>
                                        <span class="stage-kind">{kind}</span>
                                        <span class=status_class>{status}</span>
                                        <span class="stage-elapsed">{elapsed}</span>
                                        <span class="stage-headline">{headline}</span>
                                    </li>
                                }
                            }).collect_view()}
                        </ul>
                    </section>
                }.into_any()
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

            {move || match action_error.get() {
                Some(e) => view! {
                    <div class="lineage-action-error" role="alert">
                        <strong>"Action failed: "</strong>{e}
                    </div>
                }.into_any(),
                None => view! { <span></span> }.into_any(),
            }}

            <div class="lineage-detail-actions">
                <button
                    class="secondary-btn"
                    on:click=fork_click
                    title="Bundle this lineage into a portable .evolvo-bundle (I-P4: fork into a new app)"
                >
                    "Fork…"
                </button>
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
                    prop:disabled=!can_resume
                    on:click=resume_click
                    title="Pick up the multi-stage pipeline from the first non-green stage, reusing the existing worktree and plan.json"
                >
                    "Resume"
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

/// Bump `reload` on a fixed interval until the component unmounts. Used by
/// `LineagePage` so that (a) status changes from the background claude run
/// surface without user action and (b) manual edits to `lineage_jobs/*.json`
/// on disk are picked up without an app restart. `setInterval` keeps firing
/// forever unless we clear it, so register an `on_cleanup` to stop when the
/// Lineage view is unmounted.
fn spawn_interval(reload: RwSignal<u32>, interval_ms: i32) {
    let Some(win) = web_sys::window() else {
        return;
    };
    let cb = Closure::<dyn FnMut()>::new(move || {
        reload.update(|v| *v = v.wrapping_add(1));
    });
    let handle = win
        .set_interval_with_callback_and_timeout_and_arguments_0(
            cb.as_ref().unchecked_ref(),
            interval_ms,
        )
        .ok();
    // The closure must outlive the interval; `forget` leaks it for the life
    // of the page (one per LineagePage mount — bounded).
    cb.forget();
    on_cleanup(move || {
        if let Some(h) = handle {
            if let Some(win) = web_sys::window() {
                win.clear_interval_with_handle(h);
            }
        }
    });
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
        let _ = win.set_timeout_with_callback_and_timeout_and_arguments_0(cb.unchecked_ref(), ms);
    }
}
