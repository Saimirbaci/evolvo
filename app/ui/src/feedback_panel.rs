use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    window, CanvasRenderingContext2d, Element, HtmlCanvasElement, HtmlElement, HtmlImageElement,
    HtmlInputElement, HtmlTextAreaElement, PointerEvent,
};

use crate::canvas::{CanvasController, History, Shape};
use crate::design_bar::{DesignBar, DesignBindings};
use crate::desktop_overlay::DesktopCaptureContext;
use crate::interop;
use crate::types::{
    AgentAvailability, AgentKind, EditableFeedbackScreen, FeedbackRecord, FeedbackScreenPayload,
    FeedbackType, LocalDelegateAvailability, LocalDelegateConfig, SubmitFeedbackPayload,
};
use crate::voice::{VoiceRecorder, VoiceState};

const MAX_STORYBOARD_SCREENS: usize = 8;

#[derive(Debug, Clone)]
pub struct FeedbackRevisionRequest {
    pub feedback_id: String,
    pub agent: AgentKind,
}

#[derive(Clone)]
pub(crate) struct StoryboardScreenDraft {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) intended_route: String,
    pub(crate) shapes: Vec<Shape>,
    pub(crate) history: History,
    pub(crate) pasted_base64: Vec<String>,
    pub(crate) annotation_data_url: Option<String>,
}

impl StoryboardScreenDraft {
    pub(crate) fn empty(number: usize, route: String) -> Self {
        Self {
            id: format!("screen-{number}"),
            name: format!("Screen {number}"),
            description: String::new(),
            intended_route: route,
            shapes: Vec::new(),
            history: History::default(),
            pasted_base64: Vec::new(),
            annotation_data_url: None,
        }
    }

    pub(crate) fn has_signal(&self) -> bool {
        !self.shapes.is_empty() || !self.pasted_base64.is_empty()
    }

    pub(crate) fn from_editable(screen: EditableFeedbackScreen) -> Self {
        let (shapes, pasted_base64) = CanvasController::restore_annotations(
            &screen.annotations,
            &screen.pasted_image_data_urls,
        );
        Self {
            id: screen.id,
            name: screen.name,
            description: screen.description,
            intended_route: screen.intended_route,
            shapes,
            history: History::default(),
            pasted_base64,
            annotation_data_url: None,
        }
    }
}

fn save_active_screen(
    controller: &CanvasController,
    screens: RwSignal<Vec<StoryboardScreenDraft>>,
    active: RwSignal<usize>,
) {
    let idx = active.get_untracked();
    screens.update(|items| {
        if let Some(screen) = items.get_mut(idx) {
            screen.shapes = controller.shapes.get_untracked();
            screen.history = controller.history.get_untracked();
            screen.pasted_base64 = CanvasController::pasted_base64_for_shapes(&screen.shapes);
            screen.annotation_data_url = controller.to_png_data_url();
        }
    });
}

pub(crate) fn load_active_screen(controller: &CanvasController, screen: &StoryboardScreenDraft) {
    // `load` also re-derives the thumbnails and clears any in-flight drag
    // or inline text edit that belonged to the previous screen.
    controller.load(screen.shapes.clone(), screen.history.clone());
}

/// "Target: <app>" line shown at the top of the panel in the desktop
/// overlay so the user sees which app (and repo) the feedback is about.
#[component]
fn DesktopTargetBanner(ctx: DesktopCaptureContext) -> impl IntoView {
    view! {
        <div class="desktop-target" role="status">
            {move || match ctx.session.get().and_then(|s| s.target_app) {
                Some(app) => {
                    let title = if app.window_title.trim().is_empty() {
                        String::new()
                    } else {
                        format!(" — {}", app.window_title)
                    };
                    let repo = match (&app.based_on_job, &app.repo_path) {
                        (Some(job), _) => format!("Continues the build from {job}"),
                        (None, Some(p)) => format!("Source: {p}"),
                        (None, None) => {
                            "Source: Evolvo finds and clones it after you send".to_string()
                        }
                    };
                    view! {
                        <div class="desktop-target-app">
                            <span class="desktop-target-label">"Target"</span>
                            <strong>{app.name.clone()}</strong>
                            <span class="desktop-target-title">{title}</span>
                        </div>
                        <div
                            class="desktop-target-repo"
                            class:desktop-target-unresolved=app.repo_path.is_none()
                        >
                            {repo}
                        </div>
                    }
                    .into_any()
                }
                None => view! {
                    <div class="desktop-target-app">
                        <span class="desktop-target-label">"Target"</span>
                        <strong>"Unknown app"</strong>
                    </div>
                }
                .into_any(),
            }}
        </div>
    }
}

#[component]
pub fn FeedbackPanel(
    controller: CanvasController,
    route: RwSignal<String>,
    is_open: RwSignal<bool>,
    revision_request: RwSignal<Option<FeedbackRevisionRequest>>,
) -> impl IntoView {
    let feedback_type = RwSignal::new(FeedbackType::Bug);
    let agent = RwSignal::new(AgentKind::ClaudeCode);
    // Per-agent availability snapshot; `None` = still loading, empty vec =
    // backend probe failed (in which case we optimistically enable every
    // chip so the feature isn't silently disabled by a probe bug).
    let availability: RwSignal<Option<Vec<AgentAvailability>>> = RwSignal::new(None);
    // Local-delegate availability: pi binary + Ollama health. None until
    // the host responds. Greys out the "Local worker" chip when either
    // piece is missing so the user knows what to install.
    let delegate_avail: RwSignal<Option<LocalDelegateAvailability>> = RwSignal::new(None);
    // User's opt-in toggle for delegation. Only meaningful when both the
    // selected agent supports it (Claude / Codex today) AND
    // `delegate_avail` reports ready. Defaults off so the historical
    // "supervisor does everything" path is unchanged.
    let delegate_on = RwSignal::new(false);
    let text = RwSignal::new(String::new());
    let active_screen = RwSignal::new(0usize);
    let next_screen_number = RwSignal::new(2usize);
    let screens = RwSignal::new(vec![StoryboardScreenDraft::empty(1, route.get_untracked())]);
    let submitting = RwSignal::new(false);
    let status: RwSignal<Option<Result<String, String>>> = RwSignal::new(None);
    let voice = VoiceState::new();
    let revision_parent: RwSignal<Option<FeedbackRecord>> = RwSignal::new(None);
    // Auto-evolve = skip the manual "Evolve" click and fire the agent
    // run immediately on submit. Pre-checked for NewApp (where the user
    // almost always wants the agent to start straight away) and unchecked
    // for everything else so a misclick on Bug/Confusion/etc. doesn't
    // unintentionally burn agent tokens. Whenever the user changes the
    // feedback-type chip we re-apply that default — but we honour an
    // explicit toggle: once the user has clicked the checkbox themselves,
    // their choice is sticky for the rest of this panel session.
    let auto_evolve = RwSignal::new(false);
    let auto_evolve_user_set = RwSignal::new(false);
    // Desktop overlay only: start the new version of the app as soon as it
    // is built. Off by default — the user is notified and runs it from the
    // Evolvo menu bar (or the Lineage page) when they're ready.
    let auto_launch = RwSignal::new(false);
    // Plan-review gate: only meaningful for NewApp submissions (the
    // multi-stage pipeline). Off by default — checking it makes the
    // pipeline park in `Planned` after the backend plan validates so the
    // user can read plan.json before implementation burns a worktree.
    let pause_after_plan = RwSignal::new(false);
    // Present only when this panel is mounted by the desktop overlay: the
    // frozen frame replaces the in-app window capture and the target app
    // rides along on the submission.
    let desktop = use_context::<DesktopCaptureContext>();

    // A Lineage action opens this panel with a one-shot request. Hydration
    // restores the complete storyboard and then clears the request while
    // retaining the immutable parent record as submission ancestry.
    {
        let ctrl = controller.clone();
        let voice = voice.clone();
        Effect::new(move |_| {
            let Some(request) = revision_request.get() else {
                return;
            };
            revision_request.set(None);
            status.set(Some(Ok(format!(
                "Loading {} for revision…",
                request.feedback_id
            ))));
            let ctrl = ctrl.clone();
            let voice = voice.clone();
            spawn_local(async move {
                match interop::load_feedback_for_revision(&request.feedback_id).await {
                    Ok(Some(editable)) => {
                        let parent = editable.feedback;
                        let drafts: Vec<StoryboardScreenDraft> = editable
                            .screens
                            .into_iter()
                            .map(StoryboardScreenDraft::from_editable)
                            .collect();
                        let drafts = if drafts.is_empty() {
                            vec![StoryboardScreenDraft::empty(1, parent.page_route.clone())]
                        } else {
                            drafts
                        };
                        feedback_type.set(parent.feedback_type);
                        agent.set(request.agent.runnable());
                        text.set(parent.feedback_text.clone());
                        route.set(parent.page_route.clone());
                        voice.clear();
                        if let Some(transcript) = parent.voice_transcript.clone() {
                            voice.transcript.set(transcript);
                        }
                        auto_evolve.set(false);
                        // Revisions should never unexpectedly start a second
                        // agent while the parent iteration may still run.
                        // The user can explicitly re-enable the toggle.
                        auto_evolve_user_set.set(true);
                        pause_after_plan.set(false);
                        delegate_on.set(false);
                        next_screen_number.set(drafts.len().saturating_add(1));
                        active_screen.set(0);
                        screens.set(drafts.clone());
                        load_active_screen(&ctrl, &drafts[0]);
                        status.set(None);
                        revision_parent.set(Some(parent));
                    }
                    Ok(None) => status.set(Some(Err(format!(
                        "Feedback {} no longer exists",
                        request.feedback_id
                    )))),
                    Err(error) => {
                        status.set(Some(Err(format!("Could not restore feedback: {error}"))))
                    }
                }
            });
        });
    }
    Effect::new(move |_| {
        let ft = feedback_type.get();
        if !auto_evolve_user_set.get_untracked() {
            // Desktop overlay: "draw on any app" means find the app's repo and
            // start building straight away, so auto-evolve defaults on.
            auto_evolve.set(desktop.is_some() || matches!(ft, FeedbackType::NewApp));
        }
    });

    // Probe agent availability on mount. We only do this once per panel
    // lifecycle; the backend caches results so multiple opens are cheap.
    Effect::new(move |already: Option<()>| {
        if already.is_some() {
            return;
        }
        spawn_local(async move {
            match interop::list_available_agents().await {
                Ok(list) => availability.set(Some(list)),
                Err(_) => availability.set(Some(Vec::new())),
            }
        });
    });

    // Probe local-delegate availability (pi binary + Ollama health). One
    // shot per panel lifecycle, same as the agent probe; backend caches.
    Effect::new(move |already: Option<()>| {
        if already.is_some() {
            return;
        }
        spawn_local(async move {
            if let Ok(a) = interop::list_local_delegate_availability().await {
                delegate_avail.set(Some(a));
            }
        });
    });

    // Delegation only makes sense for supervisor-style agents that can
    // call into a Bash tool and have been wired for it: Claude and Codex.
    // Grok ignores the env var, so the chip would be misleading there.
    let agent_supports_delegate =
        move || matches!(agent.get(), AgentKind::ClaudeCode | AgentKind::CodexCli);
    let delegate_ready = move || {
        delegate_avail
            .get()
            .as_ref()
            .map(|a| a.ready())
            .unwrap_or(false)
    };
    let delegate_label = move || {
        delegate_avail
            .get()
            .map(|a| format!("pi · {}", a.default_model))
            .unwrap_or_else(|| "pi · gemma4".into())
    };
    let delegate_tooltip = move || match delegate_avail.get() {
        None => "Probing local worker availability…".to_string(),
        Some(a) if a.ready() => format!(
            "Delegate boilerplate to {}/{} to save supervisor tokens. Free, runs offline.",
            a.provider, a.default_model
        ),
        Some(a) if !a.pi_installed => {
            "Install `pi` (pi-agent) on PATH to enable local delegation.".into()
        }
        Some(_) => "Start Ollama (127.0.0.1:11434) to enable local delegation.".into(),
    };
    // If the user toggled delegation on but then switched to an
    // unsupported agent (e.g. Grok), drop the toggle so the submit
    // payload doesn't carry an inert `local_delegate`. Same for the case
    // where availability flips from Some(ready) to Some(not ready).
    Effect::new(move |_| {
        if delegate_on.get_untracked() && !(agent_supports_delegate() && delegate_ready()) {
            delegate_on.set(false);
        }
    });

    let submit = {
        let ctrl = controller.clone();
        let voice = voice.clone();
        move |_| {
            let is_storyboard = matches!(feedback_type.get_untracked(), FeedbackType::NewApp);
            if is_storyboard {
                save_active_screen(&ctrl, screens, active_screen);
            }
            let storyboard_has_signal = is_storyboard
                && screens
                    .get_untracked()
                    .iter()
                    .any(StoryboardScreenDraft::has_signal);
            let ready = !text.get_untracked().trim().is_empty()
                || !ctrl.shapes.get_untracked().is_empty()
                || !ctrl.pasted_base64.get_untracked().is_empty()
                || storyboard_has_signal;
            if !ready || submitting.get_untracked() {
                return;
            }
            submitting.set(true);
            status.set(None);
            let anno_url = ctrl.to_png_data_url();
            let annotations = ctrl.annotations_json();
            let pasted = CanvasController::pasted_base64_for_shapes(&ctrl.shapes.get_untracked());
            let storyboard_drafts = if is_storyboard {
                screens.get_untracked()
            } else {
                Vec::new()
            };
            let (win_w, win_h) = window_size();
            let transcript = {
                let t = voice.transcript.get_untracked();
                if t.trim().is_empty() {
                    None
                } else {
                    Some(t)
                }
            };
            let feedback_type_v = feedback_type.get_untracked();
            let agent_v = agent.get_untracked();
            let auto_evolve_v = auto_evolve.get_untracked();
            let auto_launch_v = auto_launch.get_untracked();
            let pause_after_plan_v = pause_after_plan.get_untracked();
            let parent_feedback_id_v = revision_parent.get_untracked().map(|parent| parent.id);
            // Snapshot delegate config at submit time. Only rides
            // through when the user explicitly toggled it on AND the
            // chip was actually live (probe ready, agent supports it).
            let delegate_v: Option<LocalDelegateConfig> = if delegate_on.get_untracked() {
                delegate_avail.get_untracked().map(|a| LocalDelegateConfig {
                    provider: a.provider,
                    model: a.default_model,
                })
            } else {
                None
            };
            let route_v = route.get_untracked();
            let text_v = text.get_untracked();
            let voice_b64 = voice.base64.get_untracked();
            let voice_mime = voice.mime_type.get_untracked();
            let desktop_session = desktop.and_then(|d| d.session.get_untracked());
            let desktop_session_present = desktop_session.is_some();

            let ctrl_reset = ctrl.clone();
            let voice_reset = voice.clone();
            spawn_local(async move {
                let page_b64 = match desktop_session.as_ref() {
                    // Desktop overlay: the frame was frozen before the
                    // overlay appeared, so there is nothing to hide.
                    Some(session) => Some(session.screenshot_base64.clone()),
                    None => {
                        // Hide overlays so the screenshot captures the underlying
                        // page without the canvas surface or this panel obscuring it.
                        let hidden = hide_overlays_for_capture();
                        wait_two_frames().await;
                        let page_b64 = interop::capture_window_png().await.ok();
                        restore_overlays(&hidden);
                        page_b64
                    }
                };
                // Keep the clean frame too, so "Revise on Canvas" can reopen the
                // original screen without the old strokes baked in.
                let background_base64 = desktop_session
                    .as_ref()
                    .map(|session| session.screenshot_base64.clone());
                let target_app = desktop_session
                    .as_ref()
                    .and_then(|session| session.target_app.clone());
                let frame_rect = desktop_session.as_ref().and_then(|_| desktop_frame_rect());

                // Composite: page as backdrop, annotation PNG stretched on top.
                let screenshot = match (page_b64.as_deref(), anno_url.as_deref()) {
                    (Some(page), Some(anno)) => {
                        composite_page_and_annotations(page, anno, frame_rect)
                            .await
                            .or_else(|| Some(page.to_string()))
                    }
                    (Some(page), None) => Some(page.to_string()),
                    (None, Some(anno)) => Some(
                        anno.split(',')
                            .nth(1)
                            .map(str::to_string)
                            .unwrap_or_default(),
                    ),
                    (None, None) => None,
                }
                .filter(|s| !s.is_empty());

                let mut storyboard_payload = Vec::with_capacity(storyboard_drafts.len());
                for draft in storyboard_drafts {
                    let screen_screenshot =
                        match (page_b64.as_deref(), draft.annotation_data_url.as_deref()) {
                            (Some(page), Some(anno)) => {
                                composite_page_and_annotations(page, anno, frame_rect)
                                    .await
                                    .or_else(|| Some(page.to_string()))
                            }
                            (Some(page), None) => Some(page.to_string()),
                            (None, Some(anno)) => anno
                                .split(',')
                                .nth(1)
                                .map(str::to_string)
                                .filter(|value| !value.is_empty()),
                            (None, None) => None,
                        };
                    storyboard_payload.push(FeedbackScreenPayload {
                        id: draft.id,
                        name: draft.name,
                        description: draft.description,
                        intended_route: draft.intended_route,
                        annotations: CanvasController::annotations_for_shapes(&draft.shapes),
                        pasted_images_base64: draft.pasted_base64,
                        screenshot_base64: screen_screenshot,
                        window_width: win_w,
                        window_height: win_h,
                    });
                }
                let (legacy_annotations, legacy_pasted, legacy_screenshot) = storyboard_payload
                    .first()
                    .map(|screen| {
                        (
                            screen.annotations.clone(),
                            screen.pasted_images_base64.clone(),
                            screen.screenshot_base64.clone(),
                        )
                    })
                    .unwrap_or((annotations, pasted, screenshot));

                let payload = SubmitFeedbackPayload {
                    feedback_type: feedback_type_v,
                    page_route: route_v,
                    feedback_text: text_v,
                    annotations: legacy_annotations,
                    pasted_images_base64: legacy_pasted,
                    screenshot_base64: legacy_screenshot,
                    screens: storyboard_payload,
                    voice_base64: voice_b64,
                    voice_mime_type: voice_mime,
                    voice_transcript: transcript,
                    window_width: win_w,
                    window_height: win_h,
                    agent: Some(agent_v),
                    auto_evolve: auto_evolve_v,
                    pause_after_plan: pause_after_plan_v,
                    parent_feedback_id: parent_feedback_id_v.clone(),
                    local_delegate: delegate_v,
                    target_app,
                    background_base64,
                    auto_launch: desktop_session_present && auto_launch_v,
                };

                match interop::submit_feedback(&payload).await {
                    Ok(record) => {
                        let msg = if parent_feedback_id_v.is_some() {
                            format!("Revision sent • queued as {}", record.id)
                        } else if auto_evolve_v {
                            format!("Sent • agent running for {}", record.id)
                        } else {
                            format!("Sent • queued as {}", record.id)
                        };
                        status.set(Some(Ok(msg)));
                        ctrl_reset.reset();
                        screens.set(vec![StoryboardScreenDraft::empty(1, route.get_untracked())]);
                        active_screen.set(0);
                        next_screen_number.set(2);
                        voice_reset.clear();
                        text.set(String::new());
                        revision_parent.set(None);
                        is_open.set(false);
                    }
                    Err(err) => {
                        status.set(Some(Err(err)));
                    }
                }
                submitting.set(false);
            });
        }
    };

    let disabled_ctrl = controller.clone();
    let design_ctrl = controller.clone();
    let thumbs_ctrl = controller.clone();
    let voice_view = voice.clone();

    let switch_to_screen = {
        let ctrl = controller.clone();
        move |idx: usize| {
            if idx == active_screen.get_untracked() {
                return;
            }
            save_active_screen(&ctrl, screens, active_screen);
            if let Some(screen) = screens.get_untracked().get(idx).cloned() {
                active_screen.set(idx);
                load_active_screen(&ctrl, &screen);
            }
        }
    };

    let add_screen = {
        let ctrl = controller.clone();
        move |_| {
            if screens.get_untracked().len() >= MAX_STORYBOARD_SCREENS {
                return;
            }
            save_active_screen(&ctrl, screens, active_screen);
            let number = next_screen_number.get_untracked();
            next_screen_number.set(number + 1);
            let mut items = screens.get_untracked();
            let route = format!("/screen-{number}");
            items.push(StoryboardScreenDraft::empty(number, route));
            let idx = items.len() - 1;
            let screen = items[idx].clone();
            screens.set(items);
            active_screen.set(idx);
            load_active_screen(&ctrl, &screen);
        }
    };

    let duplicate_screen = {
        let ctrl = controller.clone();
        move |_| {
            if screens.get_untracked().len() >= MAX_STORYBOARD_SCREENS {
                return;
            }
            save_active_screen(&ctrl, screens, active_screen);
            let idx = active_screen.get_untracked();
            let number = next_screen_number.get_untracked();
            next_screen_number.set(number + 1);
            let mut items = screens.get_untracked();
            let Some(mut copy) = items.get(idx).cloned() else {
                return;
            };
            copy.id = format!("screen-{number}");
            copy.name = format!("{} copy", copy.name);
            copy.intended_route = format!("/screen-{number}");
            items.insert(idx + 1, copy.clone());
            screens.set(items);
            active_screen.set(idx + 1);
            load_active_screen(&ctrl, &copy);
        }
    };

    let delete_screen = {
        let ctrl = controller.clone();
        move |_| {
            let mut items = screens.get_untracked();
            if items.len() <= 1 {
                return;
            }
            let idx = active_screen.get_untracked().min(items.len() - 1);
            items.remove(idx);
            let next = idx.min(items.len() - 1);
            let screen = items[next].clone();
            screens.set(items);
            active_screen.set(next);
            load_active_screen(&ctrl, &screen);
        }
    };

    let move_screen = {
        let ctrl = controller.clone();
        move |direction: i32| {
            save_active_screen(&ctrl, screens, active_screen);
            let mut items = screens.get_untracked();
            let from = active_screen.get_untracked();
            let to = (from as i32 + direction).clamp(0, items.len() as i32 - 1) as usize;
            if from == to {
                return;
            }
            items.swap(from, to);
            let screen = items[to].clone();
            screens.set(items);
            active_screen.set(to);
            load_active_screen(&ctrl, &screen);
        }
    };

    let offset: RwSignal<(f64, f64)> = RwSignal::new((0.0, 0.0));
    let drag_origin: RwSignal<Option<(f64, f64, f64, f64)>> = RwSignal::new(None);
    // Explicit size overrides. `None` means "use the CSS default" (width 320px,
    // height stretched between top:20 and bottom:20). Once the user drags the
    // resize handle we switch to fixed pixel dimensions.
    let size: RwSignal<Option<(f64, f64)>> = RwSignal::new(None);
    let resize_origin: RwSignal<Option<(f64, f64, f64, f64)>> = RwSignal::new(None);

    let on_resize_down = move |ev: PointerEvent| {
        ev.prevent_default();
        ev.stop_propagation();
        if let Some(target) = ev
            .current_target()
            .and_then(|t| t.dyn_into::<Element>().ok())
        {
            let _ = target.set_pointer_capture(ev.pointer_id());
        }
        let (w, h) = size.get_untracked().unwrap_or_else(current_panel_size);
        resize_origin.set(Some((ev.client_x() as f64, ev.client_y() as f64, w, h)));
    };
    let on_resize_move = move |ev: PointerEvent| {
        if let Some((sx, sy, w0, h0)) = resize_origin.get_untracked() {
            // Panel is anchored top/right, so dragging left grows width and
            // dragging down grows height.
            let dx = sx - ev.client_x() as f64;
            let dy = ev.client_y() as f64 - sy;
            let w = (w0 + dx).clamp(260.0, 900.0);
            let h = (h0 + dy).clamp(240.0, 1400.0);
            size.set(Some((w, h)));
        }
    };
    let on_resize_up = move |ev: PointerEvent| {
        resize_origin.set(None);
        if let Some(target) = ev
            .current_target()
            .and_then(|t| t.dyn_into::<Element>().ok())
        {
            let _ = target.release_pointer_capture(ev.pointer_id());
        }
    };

    let on_handle_down = move |ev: PointerEvent| {
        // Don't start a drag if the user clicked an interactive child
        // (the close button). Only grab drags that start on the header
        // element itself or the title span.
        if let Some(t) = ev.target().and_then(|t| t.dyn_into::<Element>().ok()) {
            let tag = t.tag_name();
            if tag.eq_ignore_ascii_case("button") {
                return;
            }
        }
        ev.prevent_default();
        if let Some(target) = ev
            .current_target()
            .and_then(|t| t.dyn_into::<Element>().ok())
        {
            let _ = target.set_pointer_capture(ev.pointer_id());
        }
        let (ox, oy) = offset.get_untracked();
        drag_origin.set(Some((ev.client_x() as f64, ev.client_y() as f64, ox, oy)));
    };
    let on_handle_move = move |ev: PointerEvent| {
        if let Some((sx, sy, ox, oy)) = drag_origin.get_untracked() {
            offset.set((
                ox + ev.client_x() as f64 - sx,
                oy + ev.client_y() as f64 - sy,
            ));
        }
    };
    let on_handle_up = move |ev: PointerEvent| {
        drag_origin.set(None);
        if let Some(target) = ev
            .current_target()
            .and_then(|t| t.dyn_into::<Element>().ok())
        {
            let _ = target.release_pointer_capture(ev.pointer_id());
        }
    };
    let revision_ctrl = controller.clone();
    let revision_voice = voice.clone();

    view! {
        <aside
            id="feedback-panel"
            class="panel"
            class:panel-hidden=move || !is_open.get()
            aria-label="Feedback"
            aria-hidden=move || if is_open.get() { "false" } else { "true" }
            style:transform=move || {
                let (x, y) = offset.get();
                format!("translate({x}px, {y}px)")
            }
            style:width=move || size.get().map(|(w, _)| format!("{w}px")).unwrap_or_default()
            style:height=move || size.get().map(|(_, h)| format!("{h}px")).unwrap_or_default()
            style:bottom=move || if size.get().is_some() { "auto".to_string() } else { String::new() }
        >
            <div
                class="panel-header panel-drag-handle"
                title="Drag to move"
                on:pointerdown=on_handle_down
                on:pointermove=on_handle_move
                on:pointerup=on_handle_up
                on:pointercancel=on_handle_up
            >
                <div class="panel-title">
                    {move || if revision_parent.get().is_some() { "Revise feedback" } else { "Feedback" }}
                </div>
                <button
                    class="panel-close-btn"
                    title="Close"
                    aria-label="Close feedback panel"
                    on:click=move |_| is_open.set(false)
                >
                    "×"
                </button>
            </div>

            <div class="panel-body">
                {move || match desktop {
                    // Desktop captures are one-shot: no saved designs and no
                    // draft autosave, which would clobber the in-app draft.
                    Some(ctx) => view! { <DesktopTargetBanner ctx=ctx /> }.into_any(),
                    None => view! {
                        <DesignBar
                            controller=design_ctrl.clone()
                            bindings=DesignBindings {
                                screens,
                                active_screen,
                                next_screen_number,
                                text,
                                feedback_type,
                                route,
                                revision_parent,
                            }
                        />
                    }.into_any(),
                }}
                {move || {
                    let ctrl = revision_ctrl.clone();
                    let voice = revision_voice.clone();
                    revision_parent.get().map(|parent| view! {
                        <div class="revision-banner" role="status">
                            <div>
                                <strong>{format!("Revision {}", parent.revision.number.saturating_add(1))}</strong>
                                <span>{format!("Based on {}. The original remains unchanged.", parent.id)}</span>
                            </div>
                            <button
                                class="revision-cancel"
                                title="Discard this restored draft and start unrelated feedback"
                                on:click=move |_| {
                                    ctrl.reset();
                                    let initial = StoryboardScreenDraft::empty(1, route.get_untracked());
                                    screens.set(vec![initial]);
                                    active_screen.set(0);
                                    next_screen_number.set(2);
                                    text.set(String::new());
                                    voice.clear();
                                    feedback_type.set(FeedbackType::Bug);
                                    auto_evolve_user_set.set(false);
                                    pause_after_plan.set(false);
                                    revision_parent.set(None);
                                    status.set(None);
                                }
                            >"Start fresh"</button>
                        </div>
                    })
                }}
                <div class="panel-section-label">"Agent"</div>
                <div class="type-chips agent-chips">
                    {AgentKind::all().into_iter().map(|ak| {
                        let is_active = move || agent.get() == ak;
                        // An agent is enabled if the backend either hasn't
                        // finished probing yet (optimistic), the probe
                        // failed (empty list, also optimistic), or the
                        // backend reports the binary as installed.
                        let is_enabled = move || match availability.get() {
                            None => true,
                            Some(list) if list.is_empty() => true,
                            Some(list) => list
                                .iter()
                                .find(|a| a.kind == ak)
                                .map(|a| a.installed)
                                .unwrap_or(true),
                        };
                        let tooltip = move || if is_enabled() {
                            format!("Use {}", ak.label())
                        } else {
                            format!("Install `{}` on PATH to enable {}", ak.binary(), ak.label())
                        };
                        view! {
                            <button
                                class="type-chip agent-chip"
                                class:active=is_active
                                class:disabled=move || !is_enabled()
                                prop:disabled=move || !is_enabled()
                                title=tooltip
                                on:click=move |_| {
                                    if is_enabled() {
                                        agent.set(ak)
                                    }
                                }
                            >
                                {ak.label()}
                            </button>
                        }
                    }).collect_view()}
                </div>

                {move || {
                    // Only render the local-worker row when (a) the
                    // selected agent supports delegation and (b) we have
                    // a probe result. Hidden for Grok
                    // and for hosts where pi+Ollama isn't reachable —
                    // the user shouldn't see a dead chip.
                    let visible = agent_supports_delegate() && delegate_avail.get().is_some();
                    if !visible {
                        return view! { <span></span> }.into_any();
                    }
                    let active = move || delegate_on.get();
                    let enabled = move || delegate_ready();
                    let label = delegate_label();
                    let tip = delegate_tooltip();
                    view! {
                        <div class="panel-section-label">"Local worker"</div>
                        <div class="type-chips agent-chips">
                            <button
                                class="type-chip agent-chip"
                                class:active=active
                                class:disabled=move || !enabled()
                                prop:disabled=move || !enabled()
                                title=tip
                                aria-pressed=move || if active() { "true" } else { "false" }
                                on:click=move |_| {
                                    if enabled() {
                                        delegate_on.set(!delegate_on.get_untracked());
                                    }
                                }
                            >
                                {label}
                            </button>
                        </div>
                    }.into_any()
                }}

                <div class="panel-section-label">"Type"</div>
                <div class="type-chips">
                    {FeedbackType::all().into_iter().map(|ft| {
                        let is_active = move || feedback_type.get() == ft;
                        view! {
                            <button
                                class="type-chip"
                                class:active=is_active
                                on:click=move |_| feedback_type.set(ft)
                            >
                                {ft.label()}
                            </button>
                        }
                    }).collect_view()}
                </div>

                {move || {
                    if !matches!(feedback_type.get(), FeedbackType::NewApp) {
                        return view! { <span></span> }.into_any();
                    }
                    let items = screens.get();
                    let current = active_screen.get().min(items.len().saturating_sub(1));
                    let active = items.get(current).cloned();
                    let can_add = items.len() < MAX_STORYBOARD_SCREENS;
                    let add = add_screen.clone();
                    let duplicate = duplicate_screen.clone();
                    let delete = delete_screen.clone();
                    let move_left = move_screen.clone();
                    let _move_right = move_screen.clone();
                    view! {
                        <section class="storyboard" aria-label="App screens">
                            <div class="storyboard-heading">
                                <div>
                                    <div class="panel-section-label">"App screens"</div>
                                    <div class="storyboard-help">"Draw each page, then submit them together."</div>
                                </div>
                                <button
                                    class="storyboard-add"
                                    prop:disabled=move || !can_add
                                    title=move || if can_add { "Add screen" } else { "Maximum 8 screens" }
                                    on:click=move |ev| add(ev)
                                >
                                    "+ Screen"
                                </button>
                            </div>
                            <div class="storyboard-strip" role="tablist" aria-label="Storyboard screens">
                                {items.into_iter().enumerate().map(|(idx, screen)| {
                                    let switch = switch_to_screen.clone();
                                    let is_active = idx == current;
                                    let detail = if screen.description.trim().is_empty() {
                                        screen.intended_route.clone()
                                    } else {
                                        screen.description.clone()
                                    };
                                    let title = format!("Open {}", screen.name);
                                    let preview = screen.annotation_data_url.clone();
                                    view! {
                                        <button
                                            class="storyboard-card"
                                            class:active=is_active
                                            role="tab"
                                            aria-selected=if is_active { "true" } else { "false" }
                                            title=title
                                            on:click=move |_| switch(idx)
                                        >
                                            <span class="storyboard-number">{idx + 1}</span>
                                            <span class="storyboard-preview" aria-hidden="true">
                                                {preview.map(|src| view! { <img src=src alt="" /> })}
                                            </span>
                                            <span class="storyboard-card-copy">
                                                <strong>{screen.name}</strong>
                                                <small>{detail}</small>
                                            </span>
                                        </button>
                                    }
                                }).collect_view()}
                            </div>
                            {active.map(|screen| view! {
                                <div class="storyboard-editor">
                                    <input
                                        class="storyboard-input"
                                        aria-label="Screen name"
                                        placeholder="Screen name"
                                        prop:value=screen.name
                                        on:input=move |ev| {
                                            let value = event_target::<HtmlInputElement>(&ev).value();
                                            let idx = active_screen.get_untracked();
                                            screens.update(|items| {
                                                if let Some(screen) = items.get_mut(idx) {
                                                    screen.name = value;
                                                }
                                            });
                                        }
                                    />
                                    <input
                                        class="storyboard-input storyboard-route"
                                        aria-label="Intended route"
                                        placeholder="/route"
                                        prop:value=screen.intended_route
                                        on:input=move |ev| {
                                            let value = event_target::<HtmlInputElement>(&ev).value();
                                            let idx = active_screen.get_untracked();
                                            screens.update(|items| {
                                                if let Some(screen) = items.get_mut(idx) {
                                                    screen.intended_route = value;
                                                }
                                            });
                                        }
                                    />
                                    <textarea
                                        class="storyboard-description"
                                        aria-label="Screen description"
                                        placeholder="What happens on this screen?"
                                        prop:value=screen.description
                                        on:input=move |ev| {
                                            let value = event_target::<HtmlTextAreaElement>(&ev).value();
                                            let idx = active_screen.get_untracked();
                                            screens.update(|items| {
                                                if let Some(screen) = items.get_mut(idx) {
                                                    screen.description = value;
                                                }
                                            });
                                        }
                                    ></textarea>
                                    <div class="storyboard-actions">
                                        <button
                                            class="storyboard-action"
                                            title="Move screen left"
                                            aria-label="Move screen left"
                                            prop:disabled=current == 0
                                            on:click=move |_| move_left(-1)
                                        >"←"</button>
                                        <button
                                            class="storyboard-action"
                                            title="Move screen right"
                                            aria-label="Move screen right"
                                            prop:disabled=current + 1 >= screens.get_untracked().len()
                                            on:click=move |_| _move_right(1)
                                        >"→"</button>
                                        <button
                                            class="storyboard-action"
                                            title="Duplicate screen"
                                            prop:disabled=move || !can_add
                                            on:click=move |ev| duplicate(ev)
                                        >"Duplicate"</button>
                                        <button
                                            class="storyboard-action danger"
                                            title="Delete screen"
                                            prop:disabled=move || screens.get().len() <= 1
                                            on:click=move |ev| delete(ev)
                                        >"Delete"</button>
                                    </div>
                                </div>
                            })}
                        </section>
                    }.into_any()
                }}

                <div class="panel-section-label">"Describe it"</div>
                <textarea
                    class="text-area"
                    placeholder="What happened? What did you expect?"
                    prop:value=move || text.get()
                    on:input=move |ev| {
                        let el = event_target::<HtmlTextAreaElement>(&ev);
                        text.set(el.value());
                    }
                ></textarea>

                <div class="panel-section-label">"Voice"</div>
                <VoiceRecorder state=voice_view />

                {move || {
                    let thumbs = thumbs_ctrl.pasted_base64.get();
                    if thumbs.is_empty() {
                        view! { <span></span> }.into_any()
                    } else {
                        let count = thumbs.len();
                        view! {
                            <div>
                                <div class="panel-section-label">{format!("Attached images ({count})")}</div>
                                <div class="paste-thumbs">
                                    {thumbs.into_iter().map(|b64| {
                                        let src = format!("data:image/png;base64,{b64}");
                                        view! { <img class="paste-thumb" src=src /> }
                                    }).collect_view()}
                                </div>
                            </div>
                        }.into_any()
                    }
                }}
            </div>

            <div class="panel-footer">
                {move || match status.get() {
                    Some(Ok(msg)) => view!{ <div class="status-line success">{msg}</div> }.into_any(),
                    Some(Err(err)) => view!{ <div class="status-line error">{err}</div> }.into_any(),
                    None => view!{ <span></span> }.into_any(),
                }}
                <label
                    class="auto-evolve-toggle"
                    title="If on, the agent starts working as soon as you submit — you don't have to switch to the Lineage tab and click Evolve."
                >
                    <input
                        type="checkbox"
                        prop:checked=move || auto_evolve.get()
                        on:change=move |ev| {
                            let target = event_target::<HtmlInputElement>(&ev);
                            auto_evolve_user_set.set(true);
                            auto_evolve.set(target.checked());
                        }
                    />
                    <span>"Start agent immediately"</span>
                </label>
                {desktop.is_some().then(|| view! {
                    <label
                        class="auto-evolve-toggle"
                        title="If on, Evolvo builds and starts the new version of this app as soon as the agent is done. If off, you get a notification and start it from the Evolvo menu bar or the Lineage page."
                    >
                        <input
                            type="checkbox"
                            prop:checked=move || auto_launch.get()
                            on:change=move |ev| {
                                let target = event_target::<HtmlInputElement>(&ev);
                                auto_launch.set(target.checked());
                            }
                        />
                        <span>"Run the new version as soon as it's ready"</span>
                    </label>
                })}
                <label
                    class="auto-evolve-toggle"
                    title="NewApp builds only: pause after the backend plan is validated so you can review plan.json (and its open questions) on the Lineage tab before any code is written. Approve the job to continue."
                >
                    <input
                        type="checkbox"
                        prop:checked=move || pause_after_plan.get()
                        on:change=move |ev| {
                            let target = event_target::<HtmlInputElement>(&ev);
                            pause_after_plan.set(target.checked());
                        }
                    />
                    <span>"Review plan first"</span>
                </label>
                <button
                    class="primary-btn"
                    prop:disabled=move || {
                        submitting.get()
                            || (text.get().trim().is_empty()
                                && disabled_ctrl.shapes.get().is_empty()
                                && disabled_ctrl.pasted_base64.get().is_empty()
                                && !(matches!(feedback_type.get(), FeedbackType::NewApp)
                                    && screens.get().iter().any(StoryboardScreenDraft::has_signal)))
                    }
                    on:click=submit
                >
                    {move || {
                        if submitting.get() {
                            "Sending…"
                        } else if revision_parent.get().is_some() {
                            "Send revision"
                        } else if auto_evolve.get() {
                            "Submit & Evolve"
                        } else {
                            "Submit to lineage"
                        }
                    }}
                </button>
            </div>
            <div
                class="panel-resize-handle"
                title="Drag to resize"
                aria-label="Resize feedback panel"
                role="separator"
                on:pointerdown=on_resize_down
                on:pointermove=on_resize_move
                on:pointerup=on_resize_up
                on:pointercancel=on_resize_up
            ></div>
        </aside>
    }
}

/// Current on-screen dimensions of any existing `.panel` element. Used as
/// the starting point the first time the user grabs the resize handle so the
/// panel doesn't jump from its CSS-driven stretched height to a fixed value.
fn current_panel_size() -> (f64, f64) {
    let Some(doc) = window().and_then(|w| w.document()) else {
        return (320.0, 480.0);
    };
    let Some(el) = doc.query_selector(".panel").ok().flatten() else {
        return (320.0, 480.0);
    };
    let rect = el.get_bounding_client_rect();
    (rect.width().max(260.0), rect.height().max(240.0))
}

/// Tracks elements we forced to `visibility: hidden` while a capture is in
/// flight, so `restore_overlays` can put them back without touching anything
/// else in the DOM.
struct HiddenHandles {
    nodes: Vec<(HtmlElement, String)>,
}

fn hide_overlays_for_capture() -> HiddenHandles {
    let mut nodes = Vec::new();
    let Some(doc) = window().and_then(|w| w.document()) else {
        return HiddenHandles { nodes };
    };
    // Hide the canvas overlay (transparent annotation layer) and this feedback
    // panel itself so the captured screenshot shows the page beneath them.
    for sel in [".canvas-overlay", ".panel"] {
        if let Ok(Some(node)) = doc.query_selector(sel) {
            if let Ok(el) = node.dyn_into::<HtmlElement>() {
                let style = el.style();
                let prior = style.get_property_value("visibility").unwrap_or_default();
                let _ = style.set_property("visibility", "hidden");
                nodes.push((el, prior));
            }
        }
    }
    HiddenHandles { nodes }
}

fn restore_overlays(h: &HiddenHandles) {
    for (el, prior) in &h.nodes {
        let style = el.style();
        if prior.is_empty() {
            let _ = style.remove_property("visibility");
        } else {
            let _ = style.set_property("visibility", prior);
        }
    }
}

/// Double-RAF: the first frame applies the visibility change, the second
/// guarantees the compositor has painted the updated frame before we take
/// the native screenshot.
async fn wait_two_frames() {
    for _ in 0..2 {
        let Some(win) = window() else { return };
        let promise = js_sys::Promise::new(&mut |resolve, _| {
            let cb = Closure::once_into_js(move || {
                let _ = resolve.call0(&JsValue::UNDEFINED);
            });
            let _ = win.request_animation_frame(cb.as_ref().unchecked_ref());
        });
        let _ = JsFuture::from(promise).await;
    }
}

/// Draw the page PNG and the annotation PNG into an offscreen canvas and
/// return the resulting base64. The annotation is drawn only over the canvas
/// element's actual bounding rect in the captured image — not stretched to
/// the full page — so strokes stay aligned with the page content they're
/// annotating (the app bar stays visible, strokes don't creep over it).
/// Where the desktop overlay displays its frozen frame (`.desktop-frame`),
/// in CSS px. Whole-screen mode fills the viewport; window mode shows one
/// window centred and scaled, so strokes must be mapped onto that rect.
fn desktop_frame_rect() -> Option<(f64, f64, f64, f64)> {
    let el = window()?
        .document()?
        .query_selector(".desktop-frame")
        .ok()
        .flatten()?;
    let r = el.get_bounding_client_rect();
    (r.width() > 0.0 && r.height() > 0.0).then(|| (r.left(), r.top(), r.width(), r.height()))
}

/// `frame_rect` (CSS px) is where `page_b64` is displayed on screen — the
/// desktop overlay passes it so the annotation layer lands on the right
/// part of the image. `None` keeps the in-app mapping (page = window).
async fn composite_page_and_annotations(
    page_b64: &str,
    anno_data_url: &str,
    frame_rect: Option<(f64, f64, f64, f64)>,
) -> Option<String> {
    let page_url = format!("data:image/png;base64,{page_b64}");
    let page_img = load_image(&page_url).await?;
    let anno_img = load_image(anno_data_url).await?;
    let pw = page_img.natural_width().max(1);
    let ph = page_img.natural_height().max(1);

    let doc = window().and_then(|w| w.document())?;
    let canvas: HtmlCanvasElement = doc
        .create_element("canvas")
        .ok()?
        .dyn_into::<HtmlCanvasElement>()
        .ok()?;
    canvas.set_width(pw);
    canvas.set_height(ph);
    let ctx: CanvasRenderingContext2d = canvas.get_context("2d").ok().flatten()?.dyn_into().ok()?;

    ctx.draw_image_with_html_image_element(&page_img, 0.0, 0.0)
        .ok()?;

    // Map the canvas-surface's CSS-pixel rect onto the captured image.
    // The capture may include native window chrome (title bar on macOS) so
    // the capture's height often exceeds `window.innerHeight * dpr` by the
    // chrome height. We scale by width ratio and offset vertically by that
    // chrome band.
    let win = window()?;
    let inner_w = win
        .inner_width()
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(1.0)
        .max(1.0);
    let inner_h = win
        .inner_height()
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(1.0)
        .max(1.0);
    let (dx, dy, dw, dh) = {
        let surface = doc
            .query_selector(".canvas-surface")
            .ok()
            .flatten()
            .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok());
        let rect = surface.as_ref().map(|el| el.get_bounding_client_rect());
        if let (Some((fx, fy, fw, fh)), Some(r)) = (frame_rect, rect.as_ref()) {
            // Desktop overlay: map the surface rect into the displayed frame
            // rect, then scale CSS px → image px. Strokes outside the frame
            // fall off the image edge (the canvas clips them).
            let kx = pw as f64 / fw;
            let ky = ph as f64 / fh;
            (
                (r.left() - fx) * kx,
                (r.top() - fy) * ky,
                r.width() * kx,
                r.height() * ky,
            )
        } else {
            let scale_x = pw as f64 / inner_w;
            let scale_y = pw as f64 / inner_w; // preserve aspect using width ratio
            let chrome_offset = (ph as f64 - inner_h * scale_y).max(0.0);
            match rect {
                Some(r) => (
                    r.left() * scale_x,
                    chrome_offset + r.top() * scale_y,
                    r.width() * scale_x,
                    r.height() * scale_y,
                ),
                None => (0.0, 0.0, pw as f64, ph as f64),
            }
        }
    };

    ctx.draw_image_with_html_image_element_and_dw_and_dh(&anno_img, dx, dy, dw, dh)
        .ok()?;

    let url = canvas.to_data_url_with_type("image/png").ok()?;
    url.split(',').nth(1).map(str::to_string)
}

async fn load_image(src: &str) -> Option<HtmlImageElement> {
    let img = HtmlImageElement::new().ok()?;
    img.set_src(src);
    let _ = JsFuture::from(img.decode()).await;
    if img.natural_width() == 0 {
        return None;
    }
    Some(img)
}

pub(crate) fn window_size() -> (u32, u32) {
    let Some(win) = window() else {
        return (0, 0);
    };
    let w = win
        .inner_width()
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let h = win
        .inner_height()
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    (w.max(0.0) as u32, h.max(0.0) as u32)
}
