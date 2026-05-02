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
use crate::types::{AppHealth, LineageJobRecord, PreviewSummary};

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

// FAB glyph approach — composite cluster (Option A from the plan).
//
// The single trigger (I-P3b) opens BOTH the Canvas overlay and the Feedback
// panel, so the icon must announce all three intake modalities — draw, voice,
// paste — not just "draw". A pencil alone reads as "annotate", which is half
// the truth and the original feedback complaint.
//
// We render a small inline-SVG cluster: a primary pencil glyph (the most
// frequent intake path), with a microphone and clipboard tucked into the
// corners. Inline SVG keeps us off the Trunk asset pipeline and lets the
// glyphs inherit `currentColor` from the FAB's foreground.
//
// On the first hover per session the FAB widens into a pill that reads
// "Feedback" for ~2.5s before collapsing back to a circle. Subsequent hovers
// stay collapsed so the affordance does not nag once the user has seen the
// label. State is local to the component (per-process) — re-showing after an
// app restart is acceptable and matches "first hover per session".
#[component]
fn FeedbackFab(controller: CanvasController, is_open: RwSignal<bool>) -> impl IntoView {
    let pill_seen: RwSignal<bool> = RwSignal::new(false);
    let pill_expanded: RwSignal<bool> = RwSignal::new(false);

    let on_mouseenter = move |_ev: web_sys::MouseEvent| {
        if pill_seen.get_untracked() || pill_expanded.get_untracked() {
            return;
        }
        pill_expanded.set(true);
        let Some(win) = web_sys::window() else { return };
        let cb = Closure::once_into_js(move || {
            pill_expanded.set(false);
            pill_seen.set(true);
        });
        let _ = win.set_timeout_with_callback_and_timeout_and_arguments_0(
            cb.unchecked_ref(),
            2500,
        );
    };

    view! {
        <button
            class="fab"
            class:fab-active=move || is_open.get()
            class:fab-pill-expand=move || pill_expanded.get() && !is_open.get()
            title=move || {
                if is_open.get() {
                    "Close canvas + feedback panel"
                } else {
                    "Open canvas + send feedback"
                }
            }
            aria-label=move || {
                if is_open.get() {
                    "Close canvas + feedback panel"
                } else {
                    "Open canvas + send feedback"
                }
            }
            aria-expanded=move || if is_open.get() { "true" } else { "false" }
            aria-controls="feedback-panel"
            on:mouseenter=on_mouseenter
            on:click=move |_| is_open.update(|v| *v = !*v)
        >
            {move || {
                if is_open.get() {
                    view! { <span class="fab-glyph fab-glyph-close" aria-hidden="true">"×"</span> }
                        .into_any()
                } else {
                    view! { <FeedbackFabGlyphs /> }.into_any()
                }
            }}
            {move || {
                if pill_expanded.get() && !is_open.get() {
                    view! { <span class="fab-label">"Feedback"</span> }.into_any()
                } else {
                    view! { <span></span> }.into_any()
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

/// Composite SVG cluster: pencil (primary intake), microphone (voice), and
/// clipboard (paste). Sized and positioned so the pencil reads as the focal
/// glyph and the smaller chips signal "this also does mic + paste". All three
/// inherit `currentColor` from `.fab` so the active/inactive backgrounds work
/// without per-glyph styling.
#[component]
fn FeedbackFabGlyphs() -> impl IntoView {
    view! {
        <span class="fab-glyph-cluster" aria-hidden="true">
            // Primary pencil (24x24 viewBox, scaled via CSS).
            <svg
                class="fab-glyph fab-glyph-pencil"
                viewBox="0 0 24 24"
                fill="currentColor"
            >
                <path d="M3 17.25V21h3.75L17.81 9.94l-3.75-3.75L3 17.25zM20.71 7.04a1 1 0 0 0 0-1.41l-2.34-2.34a1 1 0 0 0-1.41 0L15.13 5.12l3.75 3.75 1.83-1.83z"/>
            </svg>
            // Microphone chip (top-right).
            <svg
                class="fab-glyph fab-glyph-mic"
                viewBox="0 0 24 24"
                fill="currentColor"
            >
                <path d="M12 14a3 3 0 0 0 3-3V6a3 3 0 1 0-6 0v5a3 3 0 0 0 3 3zm5-3a5 5 0 0 1-10 0H5a7 7 0 0 0 6 6.92V21h2v-3.08A7 7 0 0 0 19 11h-2z"/>
            </svg>
            // Clipboard / paste chip (bottom-left).
            <svg
                class="fab-glyph fab-glyph-paste"
                viewBox="0 0 24 24"
                fill="currentColor"
            >
                <path d="M19 4h-3.18A3 3 0 0 0 13 2h-2a3 3 0 0 0-2.82 2H5a2 2 0 0 0-2 2v15a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2V6a2 2 0 0 0-2-2zm-7-.5a1 1 0 1 1-1 1 1 1 0 0 1 1-1zM19 21H5V6h2v2h10V6h2z"/>
            </svg>
        </span>
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

    view! {
        <div class="lineage-page">
            <aside class="lineage-sidebar">
                <div class="lineage-sidebar-head">
                    <h2 class="lineage-sidebar-title">"Lineage"</h2>
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
                                                format!("iter {} · {} · {}", r.iteration, r.agent.label(), format_time(r.created_at_unix_ms))
                                            } else {
                                                format!("{} · {}", r.agent.label(), format_time(r.created_at_unix_ms))
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

    // Inline "Fix" clarification editor state.
    let fix_open: RwSignal<bool> = RwSignal::new(false);
    let fix_text: RwSignal<String> = RwSignal::new(String::new());
    let fix_submitting: RwSignal<bool> = RwSignal::new(false);

    // Overflow (kebab) menu visibility. Click-toggle, not hover, so the
    // menu is keyboard-accessible (a hover-only menu fails leptos.md a11y
    // rule and traps users on touch devices).
    let overflow_open: RwSignal<bool> = RwSignal::new(false);

    // EvolveConfirmation modal state. The dry-run preview is loaded
    // lazily on Evolve click; while pending we show a skeleton. The
    // architect doc's I3 (bounded blast radius / dry-run default) is
    // implemented at this UI layer — the underlying state machine in
    // lineage.rs is unchanged. If the preview command errors we still
    // let the reviewer proceed via "Proceed without preview" so a broken
    // preview never bricks the lineage pipeline (I-P1 invariant).
    let evolve_modal_open: RwSignal<bool> = RwSignal::new(false);
    let preview_loading: RwSignal<bool> = RwSignal::new(false);
    let preview_data: RwSignal<Option<PreviewSummary>> = RwSignal::new(None);
    let preview_error: RwSignal<Option<String>> = RwSignal::new(None);
    let evolve_submitting: RwSignal<bool> = RwSignal::new(false);

    let approve_id_for_modal = approve_id.clone();
    let open_evolve_modal = move || {
        // Reset state every time the modal opens — stale preview from a
        // previous session would mislead the reviewer.
        preview_data.set(None);
        preview_error.set(None);
        evolve_submitting.set(false);
        evolve_modal_open.set(true);
        preview_loading.set(true);
        let id = approve_id_for_modal.clone();
        spawn_local(async move {
            match interop::preview_lineage_evolution(&id).await {
                Ok(summary) => preview_data.set(Some(summary)),
                Err(e) => preview_error.set(Some(e)),
            }
            preview_loading.set(false);
        });
    };

    let approve_id_for_confirm = approve_id.clone();
    let confirm_evolve = move || {
        if evolve_submitting.get_untracked() {
            return;
        }
        evolve_submitting.set(true);
        let id = approve_id_for_confirm.clone();
        action_error.set(None);
        spawn_local(async move {
            if let Err(e) = interop::approve_lineage_job(&id).await {
                action_error.set(Some(e));
            }
            evolve_submitting.set(false);
            evolve_modal_open.set(false);
            reload.update(|v| *v = v.wrapping_add(1));
        });
    };

    let do_approve = move || {
        if !can_approve {
            return;
        }
        action_error.set(None);
        open_evolve_modal();
    };
    let do_reject = move || {
        let id = reject_id.clone();
        action_error.set(None);
        spawn_local(async move {
            if let Err(e) = interop::reject_lineage_job(&id).await {
                action_error.set(Some(e));
            }
            reload.update(|v| *v = v.wrapping_add(1));
        });
    };
    let do_run = move || {
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
    let do_resume = move || {
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
    let agent_label = record.agent.label();

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
                <div class="lineage-detail-head-badges">
                    <span class="lineage-agent-badge" title="Agent that ran this job">{agent_label}</span>
                    <span class="list-card-status">{status_label}</span>
                </div>
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
                            "Reset the iteration — optionally tell the agent what went wrong so the fresh run has more context:"
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
                // Secondary: Run — preview the current iteration's app.
                <button
                    class="secondary-btn"
                    prop:disabled=!can_run
                    on:click=move |_| do_run()
                    aria-label="Run the iteration's app"
                    title="Launch the app built in this iteration's worktree"
                >
                    "Run"
                </button>

                // Overflow (kebab) — destructive / non-Evolve actions live
                // here so they don't compete visually with Evolve. Each
                // tooltip names the safety property so the reviewer knows
                // what the verb actually does (Reject is terminal etc.).
                <div class="lineage-overflow">
                    <button
                        class="secondary-btn lineage-overflow-trigger"
                        aria-haspopup="menu"
                        aria-expanded=move || if overflow_open.get() { "true" } else { "false" }
                        aria-label="More iteration actions"
                        title="More actions: Reset, Resume, Reject"
                        on:click=move |_| overflow_open.update(|v| *v = !*v)
                    >
                        "⋯"
                    </button>
                    {move || {
                        if !overflow_open.get() {
                            return view! { <span></span> }.into_any();
                        }
                        let do_reject = do_reject.clone();
                        let do_resume = do_resume.clone();
                        view! {
                            <div class="lineage-overflow-menu" role="menu">
                                <button
                                    class="lineage-overflow-item"
                                    role="menuitem"
                                    prop:disabled=!can_retry
                                    title="Reset — discards iteration worktree state and re-runs the agent from a fresh fork"
                                    on:click=move |_| {
                                        overflow_open.set(false);
                                        fix_open.update(|v| *v = !*v);
                                    }
                                >
                                    <span class="overflow-item-label">"Reset"</span>
                                    <span class="overflow-item-hint">"discards worktree state"</span>
                                </button>
                                <button
                                    class="lineage-overflow-item"
                                    role="menuitem"
                                    prop:disabled=!can_resume
                                    title="Resume — re-runs the agent against the existing worktree, picking up at the first non-green stage"
                                    on:click=move |_| {
                                        overflow_open.set(false);
                                        do_resume();
                                    }
                                >
                                    <span class="overflow-item-label">"Resume"</span>
                                    <span class="overflow-item-hint">"re-runs against existing worktree"</span>
                                </button>
                                <button
                                    class="lineage-overflow-item lineage-overflow-item-danger"
                                    role="menuitem"
                                    title="Reject — terminal, cannot be undone. Marks the lineage job as rejected for good."
                                    on:click=move |_| {
                                        overflow_open.set(false);
                                        do_reject();
                                    }
                                >
                                    <span class="overflow-item-label">"Reject"</span>
                                    <span class="overflow-item-hint">"terminal, cannot be undone"</span>
                                </button>
                            </div>
                        }.into_any()
                    }}
                </div>

                // Primary: Evolve — gated behind the dry-run modal so the
                // reviewer sees what's about to happen before committing.
                <button
                    class="primary-btn lineage-evolve-btn"
                    prop:disabled=!can_approve
                    on:click=move |_| do_approve()
                    aria-label="Evolve — plan and implement the next iteration"
                    title="Preview, then plan & implement the next iteration from this feedback"
                >
                    "Evolve"
                </button>
            </div>

            {move || {
                if !evolve_modal_open.get() {
                    return view! { <span></span> }.into_any();
                }
                let confirm = confirm_evolve.clone();
                view! {
                    <EvolveConfirmation
                        is_open=evolve_modal_open
                        loading=preview_loading
                        preview=preview_data
                        error=preview_error
                        submitting=evolve_submitting
                        on_confirm=Box::new(move || confirm())
                    />
                }.into_any()
            }}
        </div>
    }
}

#[component]
fn EvolveConfirmation(
    is_open: RwSignal<bool>,
    loading: RwSignal<bool>,
    preview: RwSignal<Option<PreviewSummary>>,
    error: RwSignal<Option<String>>,
    submitting: RwSignal<bool>,
    on_confirm: Box<dyn Fn() + 'static>,
) -> impl IntoView {
    let close = move || {
        if !submitting.get_untracked() {
            is_open.set(false);
        }
    };
    let close_for_overlay = close.clone();
    let close_for_cancel = close.clone();
    let close_for_kbd = close.clone();

    // Escape-to-cancel. Listen on the document so the modal works even
    // when focus is somewhere unexpected — e.g. the reviewer just
    // clicked the disabled Confirm button while it was loading.
    Effect::new(move |prev: Option<()>| {
        if prev.is_some() {
            return;
        }
        let close_kbd = close_for_kbd.clone();
        let cb = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(
            move |ev: web_sys::KeyboardEvent| {
                if !is_open.get_untracked() {
                    return;
                }
                if ev.key() == "Escape" {
                    close_kbd();
                }
            },
        );
        if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
            let _ = doc
                .add_event_listener_with_callback("keydown", cb.as_ref().unchecked_ref());
        }
        cb.forget();
    });

    view! {
        <div
            class="evolve-modal-overlay"
            role="dialog"
            aria-modal="true"
            aria-labelledby="evolve-modal-title"
            on:click=move |_| close_for_overlay()
        >
            <div
                class="evolve-modal"
                on:click=move |ev| ev.stop_propagation()
            >
                <header class="evolve-modal-head">
                    <h2 id="evolve-modal-title" class="evolve-modal-title">"Confirm Evolve — dry-run preview"</h2>
                    <p class="evolve-modal-subtitle">
                        "Review the planned scope below. Clicking Confirm will plan + implement the next iteration. Cancel keeps the lineage job exactly where it is."
                    </p>
                </header>

                <section class="evolve-modal-body">
                    {move || {
                        if loading.get() {
                            return view! {
                                <div class="evolve-modal-skeleton">"Loading preview…"</div>
                            }.into_any();
                        }
                        if let Some(err) = error.get() {
                            return view! {
                                <div class="evolve-modal-error" role="alert">
                                    <strong>"Preview unavailable: "</strong>{err}
                                    <p class="evolve-modal-error-hint">
                                        "You can still proceed without a preview. The state machine and any can_approve() gate still apply on the host."
                                    </p>
                                </div>
                            }.into_any();
                        }
                        let Some(p) = preview.get() else {
                            return view! { <span></span> }.into_any();
                        };
                        view! {
                            <dl class="evolve-modal-meta">
                                <div><dt>"Agent"</dt><dd>{p.agent.label()}</dd></div>
                                <div><dt>"From iteration"</dt><dd>{p.source_iteration}</dd></div>
                                <div><dt>"Next iteration"</dt><dd>{p.target_iteration}</dd></div>
                                <div><dt>"Dev port"</dt><dd>{p.target_port}</dd></div>
                            </dl>
                            <div class="evolve-modal-section">
                                <h3>"Plan summary"</h3>
                                <pre class="evolve-modal-summary">{p.plan_summary.clone()}</pre>
                            </div>
                            {if p.planned_files.is_empty() {
                                view! {
                                    <div class="evolve-modal-section">
                                        <h3>"Planned scope"</h3>
                                        <div class="evolve-modal-empty">
                                            "No concrete plan items recorded yet — proceeding will run the agent without a recorded plan."
                                        </div>
                                    </div>
                                }.into_any()
                            } else {
                                let items = p.planned_files.clone();
                                view! {
                                    <div class="evolve-modal-section">
                                        <h3>{format!("Planned scope ({} items)", items.len())}</h3>
                                        <ul class="evolve-modal-files">
                                            {items.into_iter().map(|f| view! { <li>{f}</li> }).collect_view()}
                                        </ul>
                                    </div>
                                }.into_any()
                            }}
                            {if p.notes.is_empty() {
                                view! { <span></span> }.into_any()
                            } else {
                                let notes = p.notes.clone();
                                view! {
                                    <div class="evolve-modal-section">
                                        <h3>"Notes"</h3>
                                        <ul class="evolve-modal-notes">
                                            {notes.into_iter().map(|n| view! { <li>{n}</li> }).collect_view()}
                                        </ul>
                                    </div>
                                }.into_any()
                            }}
                        }.into_any()
                    }}
                </section>

                <footer class="evolve-modal-actions">
                    <button
                        class="secondary-btn"
                        autofocus
                        prop:disabled=move || submitting.get()
                        on:click=move |_| close_for_cancel()
                    >
                        "Cancel"
                    </button>
                    <button
                        class="primary-btn evolve-modal-confirm"
                        prop:disabled=move || submitting.get() || loading.get()
                        on:click=move |_| on_confirm()
                    >
                        {move || {
                            if submitting.get() {
                                "Evolving…"
                            } else if error.get().is_some() {
                                "Proceed without preview"
                            } else {
                                "Confirm Evolve"
                            }
                        }}
                    </button>
                </footer>
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
