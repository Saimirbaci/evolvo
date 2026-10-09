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

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use leptos::prelude::*;
use leptos::task::spawn_local;
use send_wrapper::SendWrapper;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};

use crate::canvas::{CanvasController, CanvasSurface};
use crate::feedback_panel::{FeedbackPanel, FeedbackRevisionRequest};
use crate::interop;
use crate::toolbar::Toolbar;
use crate::types::{
    AppHealth, DelegationSummary, DesignPlan, FeedbackRecord, JobActivity, LineageJobRecord,
    LineageJobStatus, PreviewSummary,
};

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
    let revision_request: RwSignal<Option<FeedbackRevisionRequest>> = RwSignal::new(None);

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
                View::Lineage => view! {
                    <LineagePage
                        view_sig=view_sig
                        panel_open=panel_open
                        revision_request=revision_request
                    />
                }.into_any(),
            }}

            {move || {
                if panel_open.get() {
                    view! {
                        <div class="canvas-overlay">
                            <Toolbar controller=ctrl_toolbar.clone() />
                            <CanvasSurface controller=ctrl_canvas.clone() />
                        </div>
                    }.into_any()
                } else {
                    view! { <span></span> }.into_any()
                }
            }}

            // Keep the editor mounted while closed so a partially edited
            // multi-screen revision retains its draft and ancestry. The
            // visible canvas overlay remains conditional above.
            <FeedbackPanel
                controller=ctrl_panel.clone()
                route=route
                is_open=panel_open
                revision_request=revision_request
            />

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
        let _ = win.set_timeout_with_callback_and_timeout_and_arguments_0(cb.unchecked_ref(), 2500);
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

#[derive(Copy, Clone)]
pub struct LineageDetailState {
    pub fix_open: RwSignal<bool>,
    pub fix_text: RwSignal<String>,
    pub fix_submitting: RwSignal<bool>,
    pub overflow_open: RwSignal<bool>,
    pub evolve_modal_open: RwSignal<bool>,
    pub preview_loading: RwSignal<bool>,
    pub preview_data: RwSignal<Option<PreviewSummary>>,
    pub preview_error: RwSignal<Option<String>>,
    pub evolve_submitting: RwSignal<bool>,
}

impl LineageDetailState {
    pub fn new() -> Self {
        Self {
            fix_open: RwSignal::new(false),
            fix_text: RwSignal::new(String::new()),
            fix_submitting: RwSignal::new(false),
            overflow_open: RwSignal::new(false),
            evolve_modal_open: RwSignal::new(false),
            preview_loading: RwSignal::new(false),
            preview_data: RwSignal::new(None),
            preview_error: RwSignal::new(None),
            evolve_submitting: RwSignal::new(false),
        }
    }

    pub fn reset(&self) {
        self.fix_open.set(false);
        self.fix_text.set(String::new());
        self.overflow_open.set(false);
        self.evolve_modal_open.set(false);
        self.preview_data.set(None);
        self.preview_error.set(None);
    }
}

#[component]
fn LineagePage(
    view_sig: RwSignal<View>,
    panel_open: RwSignal<bool>,
    revision_request: RwSignal<Option<FeedbackRevisionRequest>>,
) -> impl IntoView {
    let items: RwSignal<Option<Result<Vec<LineageJobRecord>, String>>> = RwSignal::new(None);
    // Live state arrives through Tauri events. This signal is reserved for
    // initial loading and quiet recovery reconciliation.
    let reconcile = RwSignal::new(0_u32);
    let artifact_reload = RwSignal::new(0_u32);
    let selected: RwSignal<Option<String>> = RwSignal::new(None);
    // Sort order for the sidebar list. `true` = newest first (default), which
    // matches the user's mental model — recent work sits at the top.
    let newest_first: RwSignal<bool> = RwSignal::new(true);
    // Error surface for action buttons. Lives at the page level (not inside
    // `LineageDetail`) so that when `items` reloads and the detail component
    // is torn down + rebuilt, the error message persists long enough for the
    // user to read it.
    let action_error: RwSignal<Option<String>> = RwSignal::new(None);
    let action_success: RwSignal<Option<String>> = RwSignal::new(None);
    let detail_state = LineageDetailState::new();
    Effect::new(move |_: Option<()>| {
        let _ = reconcile.get();
        spawn_local(async move {
            let result = interop::list_lineage_jobs().await;
            match result {
                Ok(records) => {
                    // Do not invalidate the view when a safety pass returns
                    // the same durable snapshot.
                    let unchanged = matches!(
                        items.get_untracked(),
                        Some(Ok(existing)) if existing == records
                    );
                    if !unchanged {
                        items.set(Some(Ok(records)));
                    }
                }
                Err(error) => {
                    // Preserve an already usable snapshot across transient
                    // reconciliation errors. Initial-load failures still
                    // render through the existing error state.
                    if items.get_untracked().is_none() {
                        items.set(Some(Err(error)));
                    }
                }
            }
        });
    });

    // Register one set of listeners per mount. Each handle owns its JS
    // callback and unlisten function, including the async-unmount race.
    // `on_cleanup` is Send+Sync in Leptos even for CSR. These values are JS
    // thread-affine, so SendWrapper documents and enforces same-thread access
    // while allowing the cleanup registration to satisfy that API boundary.
    let listeners: SendWrapper<Rc<RefCell<Vec<interop::TauriEventListener>>>> =
        SendWrapper::new(Rc::new(RefCell::new(Vec::new())));
    let disposed = SendWrapper::new(Rc::new(Cell::new(false)));
    let seen_revisions = Rc::new(RefCell::new(HashMap::<String, u64>::new()));
    {
        let listeners = Rc::clone(&listeners);
        let disposed = Rc::clone(&disposed);
        let seen_revisions = Rc::clone(&seen_revisions);
        spawn_local(async move {
            let listener = interop::listen_lineage_job_upserted(move |event| {
                let mut revisions = seen_revisions.borrow_mut();
                let last = revisions.entry(event.job.id.clone()).or_default();
                if event.revision <= *last {
                    return;
                }
                *last = event.revision;
                drop(revisions);
                upsert_lineage_job(items, event.job);
            })
            .await;
            if let Ok(listener) = listener {
                if disposed.get() {
                    drop(listener);
                } else {
                    listeners.borrow_mut().push(listener);
                }
            }
        });
    }
    {
        let listeners = Rc::clone(&listeners);
        let disposed = Rc::clone(&disposed);
        spawn_local(async move {
            let listener = interop::listen_lineage_artifacts_changed(move |event| {
                if selected.get_untracked().as_deref() == Some(event.job_id.as_str()) {
                    artifact_reload.update(|value| *value = value.wrapping_add(1));
                }
            })
            .await;
            if let Ok(listener) = listener {
                if disposed.get() {
                    drop(listener);
                } else {
                    listeners.borrow_mut().push(listener);
                }
            }
        });
    }
    {
        let listeners = Rc::clone(&listeners);
        let disposed = Rc::clone(&disposed);
        spawn_local(async move {
            let listener = interop::listen_lineage_snapshot_invalidated(move |_| {
                reconcile.update(|value| *value = value.wrapping_add(1));
            })
            .await;
            if let Ok(listener) = listener {
                if disposed.get() {
                    drop(listener);
                } else {
                    listeners.borrow_mut().push(listener);
                }
            }
        });
    }
    on_cleanup(move || {
        disposed.set(true);
        listeners.borrow_mut().clear();
    });

    spawn_reconciliation(reconcile, 60_000);

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
                action_success.set(None);
                detail_state.reset();
            }
        }
        now
    });

    let import_click = move |_| {
        let Some(win) = web_sys::window() else { return };
        let Ok(Some(bundle_path)) = win.prompt_with_message("Path to .evolvo-bundle file:") else {
            return;
        };
        if bundle_path.trim().is_empty() {
            return;
        }
        let Ok(Some(target_root)) =
            win.prompt_with_message("Target workspace root for the new app (must be empty):")
        else {
            return;
        };
        if target_root.trim().is_empty() {
            return;
        }
        action_error.set(None);
        action_success.set(None);
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
                    action_success.set(Some(msg));
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
                {move || match action_error.get() {
                    Some(e) => view! {
                        <div class="lineage-action-error" role="alert" style="margin: 24px 24px 0;">
                            <strong>"Action failed: "</strong>{e}
                        </div>
                    }.into_any(),
                    None => view! { <span></span> }.into_any(),
                }}

                {move || match action_success.get() {
                    Some(msg) => view! {
                        <div class="lineage-action-success" role="status" style="margin: 24px 24px 0; padding: 12px; border-radius: 8px; background: rgba(22, 163, 74, 0.1); color: var(--success); border: 1px solid rgba(22, 163, 74, 0.2);">
                            {msg}
                        </div>
                    }.into_any(),
                    None => view! { <span></span> }.into_any(),
                }}

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
                    view! {
                        <LineageDetail
                            record=record
                            reload=reconcile
                            artifact_reload=artifact_reload
                            action_error=action_error
                            action_success=action_success
                            detail_state=detail_state
                            view_sig=view_sig
                            panel_open=panel_open
                            revision_request=revision_request
                        />
                    }.into_any()
                }}
            </section>
        </div>
    }
}

#[component]
fn LineageDetail(
    record: LineageJobRecord,
    reload: RwSignal<u32>,
    artifact_reload: RwSignal<u32>,
    // Surface for backend errors from Evolve / Reject / Run / Fix. Owned by
    // `LineagePage` so the message outlives this component's reconstruction
    // when `items` reloads right after an action fires.
    action_error: RwSignal<Option<String>>,
    action_success: RwSignal<Option<String>>,
    detail_state: LineageDetailState,
    view_sig: RwSignal<View>,
    panel_open: RwSignal<bool>,
    revision_request: RwSignal<Option<FeedbackRevisionRequest>>,
) -> impl IntoView {
    let can_approve = record.status.can_approve();
    // The primary action means different things per state: start the
    // iteration (Pending), accept a paused plan (Planned + plan-review gate),
    // or merge a finished build (BuildReady). Label it accordingly — the
    // review banner tells the user to "Approve", so a button that only ever
    // says "Evolve" reads as missing.
    let awaiting_plan_approval = matches!(record.status, LineageJobStatus::Planned)
        && record.pause_after_plan
        && record.plan_approved_at_unix_ms.is_none();
    let awaiting_merge = matches!(record.status, LineageJobStatus::BuildReady);
    // Reviewer's draft answers to the planner's open questions, keyed by
    // question text. Blank = accept the planner's guess.
    let plan_answers: RwSignal<std::collections::HashMap<String, String>> =
        RwSignal::new(std::collections::HashMap::new());
    let collect_answers = move || -> Vec<crate::types::PlanAnswerPayload> {
        plan_answers.with_untracked(|m| {
            m.iter()
                .filter(|(_, a)| !a.trim().is_empty())
                .map(|(q, a)| crate::types::PlanAnswerPayload {
                    question: q.clone(),
                    answer: a.trim().to_string(),
                })
                .collect()
        })
    };
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
    let revise_request = FeedbackRevisionRequest {
        feedback_id: record.feedback_id.clone(),
        agent: record.agent,
    };

    let fix_open = detail_state.fix_open;
    let fix_text = detail_state.fix_text;
    let fix_submitting = detail_state.fix_submitting;

    let overflow_open = detail_state.overflow_open;

    let evolve_modal_open = detail_state.evolve_modal_open;
    let preview_loading = detail_state.preview_loading;
    let preview_data = detail_state.preview_data;
    let preview_error = detail_state.preview_error;
    let evolve_submitting = detail_state.evolve_submitting;

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

    // Plan review: save any typed answers, then either approve (build as
    // planned + answers) or re-plan with them.
    let answer_id = approve_id.clone();
    let submit_plan_review = move |replan: bool| {
        if evolve_submitting.get_untracked() {
            return;
        }
        let answers = collect_answers();
        if replan && answers.is_empty() {
            action_error.set(Some(
                "Type an answer to at least one question to re-plan — or Approve plan to accept the guesses."
                    .into(),
            ));
            return;
        }
        evolve_submitting.set(true);
        action_error.set(None);
        let id = answer_id.clone();
        spawn_local(async move {
            let saved = if answers.is_empty() {
                Ok(())
            } else {
                interop::answer_plan_questions(&id, answers, replan)
                    .await
                    .map(|_| ())
            };
            let result = match saved {
                Ok(()) if !replan => interop::approve_lineage_job(&id).await.map(|_| ()),
                other => other,
            };
            if let Err(e) = result {
                action_error.set(Some(e));
            } else {
                plan_answers.set(std::collections::HashMap::new());
            }
            evolve_submitting.set(false);
            reload.update(|v| *v = v.wrapping_add(1));
        });
    };
    let replan_from_banner = submit_plan_review.clone();
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
    // Second handle for the plan-review "Reject plan" button; the original
    // moves into the ⋯ menu closure.
    let do_reject_plan = do_reject.clone();
    let do_run = move || {
        let id = run_id.clone();
        action_error.set(None);
        spawn_local(async move {
            if let Err(e) = interop::run_lineage_job(&id).await {
                action_error.set(Some(e));
            }
            reload.update(|v| *v = v.wrapping_add(1));
        });
    };
    let do_fork = move || {
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
        action_success.set(None);
        spawn_local(async move {
            match interop::export_lineage(&id, dest_opt.as_deref()).await {
                Ok(r) => action_success.set(Some(format!(
                    "Forked → {}. To launch as its own app: \
                     EVOLVO_WORKSPACE_ROOT=<new_dir> cargo tauri dev (after \
                     importing the bundle there).",
                    r.bundle_path
                ))),
                Err(e) => action_error.set(Some(format!("Fork failed: {e}"))),
            }
        });
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
    let feedback_record: RwSignal<Option<FeedbackRecord>> = RwSignal::new(None);
    let do_revise = move || {
        // Desktop feedback was drawn over another app's frozen screen —
        // revise it in the desktop overlay over that same frame, not on
        // Evolvo's own canvas where the screen it describes isn't visible.
        let is_desktop = feedback_record
            .get_untracked()
            .is_some_and(|f| f.target_app.is_some());
        if is_desktop {
            let request = revise_request.clone();
            spawn_local(async move {
                if let Err(e) =
                    interop::open_desktop_revision(&request.feedback_id, request.agent).await
                {
                    action_error.set(Some(e));
                }
            });
            return;
        }
        revision_request.set(Some(revise_request.clone()));
        view_sig.set(View::Home);
        panel_open.set(true);
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

    let feedback_id = record.feedback_id.clone();
    // Re-read on every lineage reload: an external app's repo resolution
    // updates the feedback's `targetApp` (resolving → resolved/failed) while
    // this detail stays open.
    Effect::new(move |_| {
        reload.track();
        artifact_reload.track();
        let feedback_id = feedback_id.clone();
        spawn_local(async move {
            if let Ok(Some(feedback)) = interop::load_feedback(&feedback_id).await {
                feedback_record.set(Some(feedback));
            }
        });
    });
    // Live agent activity: refreshed when the watcher reports a log/worktree
    // change for this job (artifact_reload) or the job record changes.
    let activity: RwSignal<Option<JobActivity>> = RwSignal::new(None);
    let activity_job_id = record.id.clone();
    Effect::new(move |_| {
        reload.track();
        artifact_reload.track();
        let id = activity_job_id.clone();
        spawn_local(async move {
            if let Ok(a) = interop::read_job_activity(&id).await {
                activity.set(Some(a));
            }
        });
    });
    let repo_input: RwSignal<String> = RwSignal::new(String::new());
    let repo_busy: RwSignal<bool> = RwSignal::new(false);
    let resolve_job_id = record.id.clone();
    let can_change_repo = matches!(
        record.status,
        LineageJobStatus::Pending | LineageJobStatus::Failed
    );
    let resolve_repo = move |repo: Option<String>| {
        if repo_busy.get_untracked() {
            return;
        }
        repo_busy.set(true);
        action_error.set(None);
        let id = resolve_job_id.clone();
        spawn_local(async move {
            match interop::resolve_target_repo(&id, repo.as_deref()).await {
                Ok(_) => repo_input.set(String::new()),
                Err(e) => action_error.set(Some(e)),
            }
            repo_busy.set(false);
            reload.update(|v| *v = v.wrapping_add(1));
        });
    };

    // Planner's open questions from plan.json — the guesses it recorded on
    // ambiguous points. Most important while the job is parked in `Planned`
    // by the plan-review gate, but shown whenever non-empty so the context
    // survives into implementation. The scoped watcher refreshes this only
    // when an artifact for the selected job changes.
    let open_questions: RwSignal<Vec<String>> = RwSignal::new(Vec::new());
    // Answers the reviewer already gave in an earlier "Answer & re-plan".
    let answered_questions: RwSignal<Vec<(String, String)>> = RwSignal::new(Vec::new());
    // Planner's design intent — layout pattern, density, theme overrides,
    // and its interpretation of ambiguous canvas marks. Lives at
    // `plan.frontend.design` in the same plan.json payload as
    // `openQuestions`; extracted from the raw JSON `Value` the same way
    // (no new Tauri command — `read_job_plan` already returns the whole
    // document).
    let design_plan: RwSignal<DesignPlan> = RwSignal::new(DesignPlan::default());
    let awaiting_plan_review = matches!(record.status, LineageJobStatus::Planned);
    let plan_id = record.id.clone();
    Effect::new(move |_| {
        let _ = artifact_reload.get();
        let id = plan_id.clone();
        spawn_local(async move {
            if let Ok(Some(plan)) = interop::read_job_plan(&id).await {
                let qs: Vec<String> = plan
                    .get("openQuestions")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|q| q.as_str())
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                open_questions.set(qs);
                let answered: Vec<(String, String)> = plan
                    .get("answeredQuestions")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|e| {
                                Some((
                                    e.get("question")?.as_str()?.to_string(),
                                    e.get("answer")?.as_str()?.to_string(),
                                ))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                answered_questions.set(answered);

                let design: DesignPlan = plan
                    .get("frontend")
                    .and_then(|f| f.get("design"))
                    .and_then(|d| serde_json::from_value(d.clone()).ok())
                    .unwrap_or_default();
                design_plan.set(design);
            }
        });
    });

    // Per-job delegation telemetry (pi-events.jsonl). `None` either
    // means "not yet loaded" or "this job didn't run delegations".
    // Refreshed from an artifact event rather than the lineage-list clock.
    let delegation: RwSignal<Option<DelegationSummary>> = RwSignal::new(None);
    let delegation_id = record.id.clone();
    Effect::new(move |_| {
        let _ = artifact_reload.get();
        let id = delegation_id.clone();
        spawn_local(async move {
            match interop::read_delegation_summary(&id).await {
                Ok(s) => delegation.set(s),
                Err(_) => delegation.set(None),
            }
        });
    });

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

            // Desktop-overlay feedback about another application: which app it
            // targets, where its source repo is (resolved automatically —
            // local checkout or cloned upstream), and a manual override.
            {move || feedback_record.get().and_then(|feedback| feedback.target_app).map(|app| {
                let title = if app.window_title.trim().is_empty() {
                    String::new()
                } else {
                    format!(" — {}", app.window_title)
                };
                let (repo_line, unresolved) = match (app.repo_status.as_str(), app.repo_path.clone()) {
                    ("resolving", _) => ("Finding the source repo… (see notes below)".to_string(), false),
                    (_, Some(p)) => {
                        let how = match app.repo_source.as_str() {
                            "executable" => "next to the running app",
                            "remembered" => "remembered",
                            "local" => "local checkout",
                            "cloned" => "cloned upstream",
                            "user" => "chosen by you",
                            _ => "resolved",
                        };
                        (format!("Source repo: {p} ({how})"), false)
                    }
                    ("failed", None) => (
                        format!(
                            "Source repo not found — {}",
                            app.repo_error.clone().unwrap_or_else(|| "unknown error".into())
                        ),
                        true,
                    ),
                    _ => ("Source repo not resolved yet — Evolve finds it first".to_string(), true),
                };
                let upstream = app.repo_url.clone();
                let resolving = app.repo_status == "resolving";
                let use_repo = resolve_repo.clone();
                let find_repo = resolve_repo.clone();
                view! {
                    <div class="lineage-target-card">
                        <div>
                            <span class="lineage-target-label">"External app"</span>
                            <strong>{app.name}</strong>
                            <span>{title}</span>
                        </div>
                        <span class="lineage-target-repo" class:lineage-target-unresolved=unresolved>
                            {repo_line}
                        </span>
                        {upstream.map(|u| view! { <span class="lineage-target-note">{format!("Upstream: {u}")}</span> })}
                        {(can_change_repo && !resolving).then(|| view! {
                            <div class="lineage-target-actions">
                                <input
                                    class="input lineage-target-input"
                                    type="text"
                                    placeholder="Local path or https git URL"
                                    aria-label="Source repo path or git URL"
                                    prop:value=move || repo_input.get()
                                    on:input=move |ev| repo_input.set(event_target_value(&ev))
                                />
                                <button
                                    class="btn"
                                    title="Use this local checkout or clone this URL as the app's source"
                                    disabled=move || repo_busy.get() || repo_input.get().trim().is_empty()
                                    on:click=move |_| use_repo(Some(repo_input.get_untracked()))
                                >
                                    "Use"
                                </button>
                                <button
                                    class="btn btn-ghost"
                                    title="Search local code folders and the web for this app's source, cloning it if needed"
                                    disabled=move || repo_busy.get()
                                    on:click=move |_| find_repo(None)
                                >
                                    "Find automatically"
                                </button>
                            </div>
                        })}
                    </div>
                }
            })}

            {move || activity.get().filter(show_activity).map(|a| view! {
                <ActivityPanel activity=a action_error=action_error reload=reload />
            })}

            {move || feedback_record.get().and_then(|feedback| {
                let revision = feedback.revision;
                if revision.number <= 1 && revision.superseded_by_feedback_id.is_none() {
                    return None;
                }
                let ancestry = revision.parent_feedback_id
                    .map(|parent| format!("Revision {} of {parent}", revision.number))
                    .unwrap_or_else(|| format!("Original feedback · revision {}", revision.number));
                Some(view! {
                    <div class="lineage-revision-card">
                        <strong>{ancestry}</strong>
                        {revision.superseded_by_feedback_id.map(|next| view! {
                            <span>{format!("Superseded by {next}")}</span>
                        })}
                    </div>
                })
            })}

            {move || {
                let qs = open_questions.get();
                let answered = answered_questions.get();
                if qs.is_empty() && answered.is_empty() {
                    return view! { <span></span> }.into_any();
                }
                let class = if awaiting_plan_review {
                    "plan-questions plan-questions-review"
                } else {
                    "plan-questions"
                };
                let hint = if awaiting_plan_approval {
                    "The plan is paused for your review. Answer any question where the planner guessed wrong (leave it blank to accept the guess), then Answer & re-plan — or Approve plan to build it as planned."
                } else if awaiting_plan_review {
                    "The plan is paused for your review. These are the guesses the planner made on ambiguous points."
                } else {
                    "Guesses the planner recorded on ambiguous points in this iteration's plan."
                };
                let replan = replan_from_banner.clone();
                view! {
                    <section class=class aria-label="Planner's open questions">
                        <h4>"Planner's open questions"</h4>
                        <p class="plan-questions-hint">{hint}</p>
                        <ul class="plan-questions-list">
                            {qs.into_iter().enumerate().map(|(idx, q)| {
                                let key = q.clone();
                                let key_input = q.clone();
                                let input_id = format!("plan-answer-{idx}");
                                view! {
                                    <li>
                                        <span class="plan-question-text">{q}</span>
                                        {awaiting_plan_approval.then(|| view! {
                                            <textarea
                                                id=input_id.clone()
                                                class="input plan-answer"
                                                rows="2"
                                                maxlength="2000"
                                                aria-label="Your answer (leave blank to accept the planner's guess)"
                                                placeholder="Your answer — leave blank to accept the planner's guess"
                                                prop:value=move || plan_answers.with(|m| m.get(&key).cloned().unwrap_or_default())
                                                on:input=move |ev| {
                                                    let value = event_target::<web_sys::HtmlTextAreaElement>(&ev).value();
                                                    let key = key_input.clone();
                                                    plan_answers.update(|m| {
                                                        if value.trim().is_empty() {
                                                            m.remove(&key);
                                                        } else {
                                                            m.insert(key, value);
                                                        }
                                                    });
                                                }
                                            ></textarea>
                                        })}
                                    </li>
                                }
                            }).collect_view()}
                        </ul>
                        {(!answered.is_empty()).then(|| view! {
                            <details class="plan-answered" open=awaiting_plan_approval>
                                <summary>{format!("Your earlier answers ({})", answered.len())}</summary>
                                <ul class="plan-questions-list">
                                    {answered.into_iter().map(|(q, a)| view! {
                                        <li>
                                            <span class="plan-question-text">{q}</span>
                                            <span class="plan-answer-given">{a}</span>
                                        </li>
                                    }).collect_view()}
                                </ul>
                            </details>
                        })}
                        {awaiting_plan_approval.then(|| {
                            let replan = replan.clone();
                            view! {
                                <div class="plan-answer-actions">
                                    <span class="plan-answer-count">
                                        {move || {
                                            let n = plan_answers.with(|m| m.len());
                                            if n == 0 {
                                                "No answers yet".to_string()
                                            } else {
                                                format!("{n} answered")
                                            }
                                        }}
                                    </span>
                                    <button
                                        class="secondary-btn plan-replan-btn"
                                        prop:disabled=move || evolve_submitting.get() || plan_answers.with(|m| m.is_empty())
                                        title="Re-run the planner with your answers. It continues straight into implementation if nothing is left to decide, otherwise it pauses here again."
                                        on:click=move |_| replan(true)
                                    >
                                        {move || if evolve_submitting.get() { "Working…" } else { "Answer & re-plan" }}
                                    </button>
                                </div>
                            }
                        })}
                    </section>
                }.into_any()
            }}

            {move || {
                let design = design_plan.get();
                if design.is_empty() {
                    return view! { <span></span> }.into_any();
                }
                let layout_badge = if design.layout_pattern.is_empty() {
                    None
                } else {
                    Some(design.layout_pattern.clone())
                };
                let density_badge = if design.density.is_empty() {
                    None
                } else {
                    Some(design.density.clone())
                };
                view! {
                    <section class="plan-design" aria-label="Planner's design intent">
                        <h4>"Design"</h4>
                        {move || {
                            if layout_badge.is_none() && density_badge.is_none() {
                                return view! { <span></span> }.into_any();
                            }
                            view! {
                                <div class="plan-design-badges">
                                    {layout_badge.clone().map(|p| view! {
                                        <span class="badge badge-accent">{p}</span>
                                    })}
                                    {density_badge.clone().map(|d| view! {
                                        <span class="badge">{d}</span>
                                    })}
                                </div>
                            }.into_any()
                        }}
                        {(!design.theme_overrides.is_empty()).then(|| view! {
                            <ul class="plan-design-tokens">
                                {design.theme_overrides.iter().cloned().map(|ov| {
                                    let reason = if ov.reason.is_empty() {
                                        None
                                    } else {
                                        Some(view! { <span class="plan-design-reason">{format!(" — {}", ov.reason)}</span> })
                                    };
                                    view! {
                                        <li>
                                            <code>{ov.token}</code>
                                            " → "
                                            <code>{ov.value}</code>
                                            {reason}
                                        </li>
                                    }
                                }).collect_view()}
                            </ul>
                        })}
                        {(!design.visual_notes.is_empty()).then(|| view! {
                            <ul class="plan-design-notes">
                                {design.visual_notes.iter().cloned().map(|n| view! {
                                    <li><strong>{n.region}</strong>{": "}{n.interpretation}</li>
                                }).collect_view()}
                            </ul>
                        })}
                    </section>
                }.into_any()
            }}

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

            {move || {
                // Hide the section completely when the job didn't run
                // any delegations. Showing zeros would just be noise.
                let Some(s) = delegation.get() else {
                    return view! { <span></span> }.into_any();
                };
                if s.total == 0 && s.capped == 0 {
                    return view! { <span></span> }.into_any();
                }
                let runtime_s = (s.runtime_ms as f64) / 1000.0;
                let head = format!(
                    "Local worker · {} runs · {} failed · {} capped · {:.1}s",
                    s.total, s.failed, s.capped, runtime_s
                );
                let by_stage = s.by_stage.clone();
                view! {
                    <section class="lineage-delegation">
                        <h4>{head}</h4>
                        {if by_stage.is_empty() {
                            view! { <span></span> }.into_any()
                        } else {
                            view! {
                                <ul class="stage-list">
                                    {by_stage.into_iter().map(|st| {
                                        let line = format!(
                                            "{} · {} runs · {} failed · {} capped · {:.1}s",
                                            st.stage,
                                            st.total,
                                            st.failed,
                                            st.capped,
                                            (st.runtime_ms as f64) / 1000.0,
                                        );
                                        view! { <li class="stage-row"><span class="stage-headline">{line}</span></li> }
                                    }).collect_view()}
                                </ul>
                            }.into_any()
                        }}
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

            <div class="lineage-detail-actions">
                <button
                    class="secondary-btn"
                    on:click=move |_| do_revise()
                    aria-label="Revise this feedback on the canvas"
                    title="Restore this iteration's drawing as a new immutable feedback revision"
                >
                    "Revise on Canvas"
                </button>
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
                        title="More actions: Fork, Reset, Resume, Reject"
                        on:click=move |_| overflow_open.update(|v| *v = !*v)
                    >
                        "⋯"
                    </button>
                    {move || {
                        if !overflow_open.get() {
                            return view! { <span></span> }.into_any();
                        }
                        let do_fork = do_fork.clone();
                        let do_reject = do_reject.clone();
                        let do_resume = do_resume.clone();
                        view! {
                            <div class="lineage-overflow-menu" role="menu">
                                <button
                                    class="lineage-overflow-item"
                                    role="menuitem"
                                    title="Fork — bundle this lineage into a portable .evolvo-bundle (I-P4)"
                                    on:click=move |_| {
                                        overflow_open.set(false);
                                        do_fork();
                                    }
                                >
                                    <span class="overflow-item-label">"Fork…"</span>
                                    <span class="overflow-item-hint">"bundle into a portable fork"</span>
                                </button>
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

                // Plan review: Reject sits next to Approve (it's otherwise
                // buried in the ⋯ menu) so both answers to the banner's
                // question are one click away.
                {
                    let do_reject = do_reject_plan.clone();
                    awaiting_plan_approval.then(|| view! {
                        <button
                            class="secondary-btn lineage-reject-plan-btn"
                            on:click=move |_| do_reject()
                            aria-label="Reject this plan"
                            title="Reject — stops this job for good. Resubmit the feedback with answers to the open questions."
                        >
                            "Reject plan"
                        </button>
                    })
                }

                // Primary action. Starting an iteration goes through the
                // dry-run modal so the reviewer sees what's about to happen;
                // approving a paused plan is a direct click — the plan and
                // its open questions are already on screen.
                {
                    let submit_plan_review = submit_plan_review.clone();
                    let (label, aria, title) = if awaiting_plan_approval {
                        (
                            "Approve plan",
                            "Approve the plan and start implementation",
                            "Start implementing — with your answers where you gave them, the planner's guesses elsewhere",
                        )
                    } else if awaiting_merge {
                        (
                            "Approve & merge",
                            "Approve the build and merge it into the source repo",
                            "Preview, then merge this iteration's branch into the source repo (checks run after merge; rolled back on failure)",
                        )
                    } else {
                        (
                            "Evolve",
                            "Evolve — plan and implement the next iteration",
                            "Preview, then plan & implement the next iteration from this feedback",
                        )
                    };
                    view! {
                        <button
                            class="primary-btn lineage-evolve-btn"
                            prop:disabled=move || !can_approve || evolve_submitting.get()
                            on:click=move |_| {
                                if awaiting_plan_approval {
                                    submit_plan_review(false);
                                } else {
                                    do_approve();
                                }
                            }
                            aria-label=aria
                            title=title
                        >
                            {move || if awaiting_plan_approval && evolve_submitting.get() { "Approving…" } else { label }}
                        </button>
                    }
                }
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
        let cb =
            Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |ev: web_sys::KeyboardEvent| {
                if !is_open.get_untracked() {
                    return;
                }
                if ev.key() == "Escape" {
                    close_kbd();
                }
            });
        if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
            let _ = doc.add_event_listener_with_callback("keydown", cb.as_ref().unchecked_ref());
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

/// Worth showing once there is something to show: a run in progress,
/// transcript lines, or changes on disk.
fn show_activity(a: &JobActivity) -> bool {
    a.running || !a.events.is_empty() || !a.changed_files.is_empty()
}

fn format_duration(ms: u64) -> String {
    let mins = ms / 60_000;
    if mins >= 60 {
        format!("{}h {}m", mins / 60, mins % 60)
    } else if mins >= 1 {
        format!("{mins}m")
    } else {
        format!("{}s", ms / 1000)
    }
}

/// Phase stepper + "now" line + recent agent actions + changed files.
#[component]
fn ActivityPanel(
    activity: JobActivity,
    action_error: RwSignal<Option<String>>,
    reload: RwSignal<u32>,
) -> impl IntoView {
    let stop_job_id = activity.job_id.clone();
    let stop = move |_| {
        let id = stop_job_id.clone();
        spawn_local(async move {
            if let Err(e) = interop::stop_lineage_run(&id).await {
                action_error.set(Some(e));
            }
            reload.update(|v| *v = v.wrapping_add(1));
        });
    };
    let app_running = activity.app_running;
    let now = js_sys::Date::now() as u64;
    let timing = match (
        activity.duration_ms,
        activity.started_at_unix_ms,
        activity.running,
    ) {
        (Some(d), _, false) => Some(format!("Agent took {}", format_duration(d))),
        (_, Some(start), true) => Some(format!(
            "Running for {}",
            format_duration(now.saturating_sub(start))
        )),
        _ => None,
    };
    let cost = activity.cost_usd.map(|c| format!("${c:.2}"));
    let meta = [timing, cost]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
    let running = activity.running;
    let now_line = activity.current.clone().filter(|_| running);
    let events_count = activity.events.len();
    let files_count = activity.changed_files.len();
    // Newest first: the reader cares about what just happened.
    let events: Vec<_> = activity.events.iter().rev().cloned().collect();
    let files = activity.changed_files.clone();
    view! {
        <section class="activity-panel" aria-label="Agent activity">
            <div class="activity-head">
                <strong>{if running { "Agent activity · live" } else { "Agent activity" }}</strong>
                <span class="activity-meta">{meta}</span>
            </div>
            <ol class="activity-steps">
                {activity.phases.into_iter().map(|p| {
                    let class = format!("activity-step activity-step-{}", p.state);
                    view! { <li class=class>{p.label}</li> }
                }).collect_view()}
            </ol>
            {now_line.map(|line| view! {
                <div class="activity-now" role="status">
                    <span class="activity-pulse" aria-hidden="true"></span>
                    <span>{line}</span>
                </div>
            })}
            {app_running.then(|| view! {
                <div class="activity-now activity-app-running" role="status">
                    <span class="activity-pulse" aria-hidden="true"></span>
                    <span>"The new version is running — try your change."</span>
                    <button
                        class="btn btn-ghost activity-stop"
                        title="Stop the version Evolvo launched for this job"
                        on:click=stop
                    >
                        "Stop"
                    </button>
                </div>
            })}
            {(events_count > 0).then(|| view! {
                <details class="activity-section" open=running>
                    <summary>{format!("Recent steps ({events_count})")}</summary>
                    <ul class="activity-feed">
                        {events.into_iter().map(|e| {
                            let icon = match (e.kind.as_str(), e.tool.as_deref()) {
                                ("message", _) => "💬",
                                ("result", _) => "✓",
                                (_, Some("Edit" | "MultiEdit" | "Write" | "NotebookEdit")) => "✎",
                                (_, Some("Bash")) => "▸",
                                (_, Some("Read" | "Grep" | "Glob")) => "◎",
                                (_, Some("WebSearch" | "WebFetch")) => "🌐",
                                _ => "·",
                            };
                            let class = format!("activity-item activity-item-{}", e.kind);
                            view! {
                                <li class=class>
                                    <span class="activity-icon" aria-hidden="true">{icon}</span>
                                    <span>{e.text}</span>
                                </li>
                            }
                        }).collect_view()}
                    </ul>
                </details>
            })}
            {(files_count > 0).then(|| view! {
                <details class="activity-section">
                    <summary>{format!("Changed files ({files_count})")}</summary>
                    <ul class="activity-files">
                        {files.into_iter().map(|f| view! { <li><code>{f}</code></li> }).collect_view()}
                    </ul>
                </details>
            })}
        </section>
    }
}

fn format_time(ms: u64) -> String {
    let date = js_sys::Date::new(&JsValue::from_f64(ms as f64));
    String::from(date.to_iso_string())
}

fn upsert_lineage_job(
    items: RwSignal<Option<Result<Vec<LineageJobRecord>, String>>>,
    incoming: LineageJobRecord,
) {
    let next = match items.get_untracked() {
        Some(Ok(mut records)) => {
            if let Some(existing) = records.iter_mut().find(|record| record.id == incoming.id) {
                // Filesystem watcher events can duplicate the immediate
                // persistence event. They can also arrive after a newer
                // write, so never regress a record by timestamp.
                if incoming.updated_at_unix_ms < existing.updated_at_unix_ms
                    || existing == &incoming
                {
                    return;
                }
                *existing = incoming;
            } else {
                records.push(incoming);
            }
            records
        }
        _ => vec![incoming],
    };
    items.set(Some(Ok(next)));
}

/// Reconcile on focus and every 60 seconds as a missed-event safety net.
/// Equality checking in the loader means an unchanged pass does not touch the
/// rendered tree. Both browser callbacks are owned until component cleanup;
/// unlike the old poller this leaves no leaked closures across route mounts.
fn spawn_reconciliation(reload: RwSignal<u32>, interval_ms: i32) {
    let Some(win) = web_sys::window() else {
        return;
    };

    let interval_cb = Closure::<dyn FnMut()>::new(move || {
        reload.update(|v| *v = v.wrapping_add(1));
    });
    let handle = win
        .set_interval_with_callback_and_timeout_and_arguments_0(
            interval_cb.as_ref().unchecked_ref(),
            interval_ms,
        )
        .ok();

    let focus_reload = reload;
    let focus_cb = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| {
        focus_reload.update(|v| *v = v.wrapping_add(1));
    });
    let focus_installed = win
        .add_event_listener_with_callback("focus", focus_cb.as_ref().unchecked_ref())
        .is_ok();

    let cleanup = SendWrapper::new((win, handle, focus_installed, interval_cb, focus_cb));
    on_cleanup(move || {
        let (win, handle, focus_installed, interval_cb, focus_cb) = cleanup.take();
        if let Some(h) = handle {
            win.clear_interval_with_handle(h);
        }
        if focus_installed {
            let _ =
                win.remove_event_listener_with_callback("focus", focus_cb.as_ref().unchecked_ref());
        }
        drop(interval_cb);
        drop(focus_cb);
    });
}
