use leptos::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{Element, PointerEvent};

use crate::canvas::{CanvasController, DrawingTool};

const SWATCHES: &[(&str, &str)] = &[
    ("Red", "#ef4444"),
    ("Orange", "#f97316"),
    ("Yellow", "#eab308"),
    ("Green", "#16a34a"),
    ("Blue", "#2563eb"),
    ("Purple", "#7c3aed"),
    ("Black", "#0f172a"),
];

const STROKES: &[(&str, f64)] = &[("S", 1.5), ("M", 3.0), ("L", 6.0)];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Popover {
    Tool,
    Color,
    More,
}

#[component]
pub fn Toolbar(controller: CanvasController) -> impl IntoView {
    let offset: RwSignal<(f64, f64)> = RwSignal::new((0.0, 0.0));
    let drag_origin: RwSignal<Option<(f64, f64, f64, f64)>> = RwSignal::new(None);
    let open: RwSignal<Option<Popover>> = RwSignal::new(None);

    let toggle = move |which: Popover| {
        open.update(|v| {
            *v = if *v == Some(which) { None } else { Some(which) };
        });
    };

    let on_handle_down = move |ev: PointerEvent| {
        ev.prevent_default();
        if let Some(target) = ev.target().and_then(|t| t.dyn_into::<Element>().ok()) {
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
        if let Some(target) = ev.target().and_then(|t| t.dyn_into::<Element>().ok()) {
            let _ = target.release_pointer_capture(ev.pointer_id());
        }
    };

    view! {
        <div
            class="toolbar"
            aria-label="Drawing tools"
            style:transform=move || {
                let (x, y) = offset.get();
                format!("translate({x}px, {y}px)")
            }
        >
            <div
                class="toolbar-handle"
                title="Drag to move toolbar"
                aria-label="Move toolbar"
                on:pointerdown=on_handle_down
                on:pointermove=on_handle_move
                on:pointerup=on_handle_up
                on:pointercancel=on_handle_up
            >
                "⋮⋮"
            </div>

            <ActiveToolButton controller=controller.clone() toggle=toggle />

            <ColorPuck controller=controller.clone() toggle=toggle />

            <StrokeCycle controller=controller.clone() />

            <button
                class="pill-btn"
                title="Undo (Cmd/Ctrl+Z)"
                aria-label="Undo"
                on:click={
                    let ctrl = controller.clone();
                    move |_| ctrl.undo()
                }
            >
                "↶"
            </button>

            <button
                class="pill-btn"
                title="More"
                aria-label="More options"
                class:active=move || open.get() == Some(Popover::More)
                on:click=move |_| toggle(Popover::More)
            >
                "⋯"
            </button>

            {
                let ctrl = controller.clone();
                move || match open.get() {
                    Some(Popover::Tool) => view! {
                        <ToolPicker
                            controller=ctrl.clone()
                            close=move || open.set(None)
                        />
                    }.into_any(),
                    Some(Popover::Color) => view! {
                        <ColorPicker
                            controller=ctrl.clone()
                            close=move || open.set(None)
                        />
                    }.into_any(),
                    Some(Popover::More) => view! {
                        <OverflowMenu
                            controller=ctrl.clone()
                            close=move || open.set(None)
                        />
                    }.into_any(),
                    None => view! { <span></span> }.into_any(),
                }
            }
        </div>
    }
}

#[component]
fn ActiveToolButton<F>(controller: CanvasController, toggle: F) -> impl IntoView
where
    F: Fn(Popover) + Clone + 'static,
{
    let ctrl_for_icon = controller.clone();
    let icon = move || tool_icon(ctrl_for_icon.active_tool.get());
    let ctrl_for_label = controller.clone();
    let label = move || ctrl_for_label.active_tool.get().label().to_string();
    view! {
        <button
            class="pill-btn pill-tool"
            title=move || format!("{} — click to change tool", label())
            aria-label=move || format!("Active tool: {}", label())
            on:click=move |_| toggle(Popover::Tool)
        >
            <span class="pill-tool-glyph">{icon}</span>
        </button>
    }
}

#[component]
fn ColorPuck<F>(controller: CanvasController, toggle: F) -> impl IntoView
where
    F: Fn(Popover) + Clone + 'static,
{
    let ctrl_for_bg = controller.clone();
    let bg = move || ctrl_for_bg.stroke_color.get();
    view! {
        <button
            class="pill-btn pill-color"
            title="Stroke color"
            aria-label="Change stroke color"
            on:click=move |_| toggle(Popover::Color)
        >
            <span class="pill-color-dot" style:background-color=bg></span>
        </button>
    }
}

#[component]
fn StrokeCycle(controller: CanvasController) -> impl IntoView {
    let ctrl_label = controller.clone();
    let label = move || {
        let w = ctrl_label.stroke_width.get();
        STROKES
            .iter()
            .min_by(|a, b| (a.1 - w).abs().partial_cmp(&(b.1 - w).abs()).unwrap())
            .map(|(l, _)| l.to_string())
            .unwrap_or_else(|| "M".into())
    };
    view! {
        <button
            class="pill-btn pill-stroke"
            title="Stroke width (click to cycle, [ / ] keys)"
            aria-label="Stroke width"
            on:click={
                let ctrl = controller.clone();
                move |_| ctrl.cycle_stroke(1)
            }
        >
            {label}
        </button>
    }
}

#[component]
fn ToolPicker<F>(controller: CanvasController, close: F) -> impl IntoView
where
    F: Fn() + Clone + 'static,
{
    let tools = DrawingTool::all();
    view! {
        <div class="popover popover-tools" role="menu">
            {tools.into_iter().map(|tool| {
                let ctrl = controller.clone();
                let is_active = move || ctrl.active_tool.get() == tool;
                let ctrl_click = controller.clone();
                let close = close.clone();
                view! {
                    <button
                        class="popover-item"
                        class:active=is_active
                        title=format!("{} ({})", tool.label(), tool.shortcut())
                        aria-label=tool.label()
                        on:click=move |_| {
                            ctrl_click.active_tool.set(tool);
                            close();
                        }
                    >
                        <span class="popover-glyph">{tool_icon(tool)}</span>
                        <span class="popover-label">{tool.label()}</span>
                        <span class="popover-kbd">{tool.shortcut()}</span>
                    </button>
                }
            }).collect_view()}
        </div>
    }
}

#[component]
fn ColorPicker<F>(controller: CanvasController, close: F) -> impl IntoView
where
    F: Fn() + Clone + 'static,
{
    view! {
        <div class="popover popover-colors" role="menu">
            {SWATCHES.iter().map(|(label, hex)| {
                let hex = hex.to_string();
                let hex_click = hex.clone();
                let hex_check = hex.clone();
                let ctrl = controller.clone();
                let is_active = move || ctrl.stroke_color.get() == hex_check;
                let ctrl_click = controller.clone();
                let close = close.clone();
                view! {
                    <button
                        class="color-swatch"
                        class:active=is_active
                        title=label.to_string()
                        aria-label=format!("Color: {label}")
                        style:background-color=hex.clone()
                        on:click=move |_| {
                            ctrl_click.stroke_color.set(hex_click.clone());
                            close();
                        }
                    ></button>
                }
            }).collect_view()}
        </div>
    }
}

#[component]
fn OverflowMenu<F>(controller: CanvasController, close: F) -> impl IntoView
where
    F: Fn() + Clone + 'static,
{
    let ctrl_clear = controller.clone();
    let close_clear = close.clone();
    let ctrl_delete = controller.clone();
    let close_delete = close.clone();
    let has_selection = {
        let ctrl = controller.clone();
        move || ctrl.selected_idx.get().is_some()
    };
    view! {
        <div class="popover popover-menu" role="menu">
            <button
                class="popover-item"
                on:click=move |_| {
                    ctrl_delete.delete_selected();
                    close_delete();
                }
                disabled=move || !has_selection()
                title="Delete selected shape (Del)"
            >
                <span class="popover-glyph">"⌦"</span>
                <span class="popover-label">"Delete selected"</span>
                <span class="popover-kbd">"Del"</span>
            </button>
            <button
                class="popover-item danger"
                on:click=move |_| {
                    ctrl_clear.clear_all();
                    close_clear();
                }
                title="Clear entire canvas"
            >
                <span class="popover-glyph">"⊘"</span>
                <span class="popover-label">"Clear canvas"</span>
            </button>
        </div>
    }
}

fn tool_icon(tool: DrawingTool) -> &'static str {
    match tool {
        DrawingTool::Select => "⇲",
        DrawingTool::Pen => "✎",
        DrawingTool::Line => "╱",
        DrawingTool::Arrow => "➚",
        DrawingTool::Rectangle => "▭",
        DrawingTool::Ellipse => "◯",
        DrawingTool::Highlight => "▬",
        DrawingTool::Text => "T",
        DrawingTool::Crop => "✂",
        DrawingTool::Eraser => "⌫",
    }
}
