use leptos::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{Element, PointerEvent};

use crate::canvas::{CanvasController, DrawingTool};

const SWATCHES: &[(&str, &str)] = &[
    ("red", "#ef4444"),
    ("orange", "#f97316"),
    ("yellow", "#eab308"),
    ("green", "#16a34a"),
    ("blue", "#2563eb"),
    ("purple", "#7c3aed"),
    ("black", "#0f172a"),
];

const STROKES: &[(&str, f64)] = &[("S", 1.5), ("M", 3.0), ("L", 6.0)];

#[component]
pub fn Toolbar(controller: CanvasController) -> impl IntoView {
    let tools = DrawingTool::all();
    let offset: RwSignal<(f64, f64)> = RwSignal::new((0.0, 0.0));
    let drag_origin: RwSignal<Option<(f64, f64, f64, f64)>> = RwSignal::new(None);

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
                class="drag-handle"
                title="Drag to move"
                aria-label="Move toolbar"
                on:pointerdown=on_handle_down
                on:pointermove=on_handle_move
                on:pointerup=on_handle_up
                on:pointercancel=on_handle_up
            >
                "⋮⋮"
            </div>
            <div class="toolbar-row">
                {tools.into_iter().map(|tool| {
                    let ctrl = controller.clone();
                    let is_active = move || ctrl.active_tool.get() == tool;
                    let ctrl_click = controller.clone();
                    view! {
                        <button
                            class="tool-btn"
                            class:active=is_active
                            title=tool.label()
                            on:click=move |_| ctrl_click.active_tool.set(tool)
                        >
                            {tool_icon(tool)}
                        </button>
                    }
                }).collect_view()}
            </div>

            <div class="toolbar-divider"></div>

            <div class="toolbar-row">
                {SWATCHES.iter().map(|(label, hex)| {
                    let hex = hex.to_string();
                    let hex_for_click = hex.clone();
                    let hex_for_check = hex.clone();
                    let ctrl = controller.clone();
                    let is_active = move || ctrl.stroke_color.get() == hex_for_check;
                    let ctrl_click = controller.clone();
                    view! {
                        <button
                            class="color-swatch"
                            class:active=is_active
                            title=label.to_string()
                            style:background-color=hex.clone()
                            on:click=move |_| ctrl_click.stroke_color.set(hex_for_click.clone())
                        ></button>
                    }
                }).collect_view()}
            </div>

            <div class="toolbar-divider"></div>

            <div class="toolbar-row">
                {STROKES.iter().map(|(label, width)| {
                    let width = *width;
                    let ctrl = controller.clone();
                    let is_active = move || (ctrl.stroke_width.get() - width).abs() < 0.01;
                    let ctrl_click = controller.clone();
                    view! {
                        <button
                            class="tool-btn stroke-btn"
                            class:active=is_active
                            title=format!("Stroke {label}")
                            on:click=move |_| ctrl_click.stroke_width.set(width)
                        >
                            {*label}
                        </button>
                    }
                }).collect_view()}
            </div>

            <div class="toolbar-divider"></div>

            <div class="toolbar-row">
                <button
                    class="tool-btn"
                    title="Undo (Cmd/Ctrl+Z)"
                    on:click={
                        let ctrl = controller.clone();
                        move |_| ctrl.undo()
                    }
                >
                    "↶"
                </button>
                <button
                    class="tool-btn"
                    title="Clear canvas"
                    on:click={
                        let ctrl = controller.clone();
                        move |_| ctrl.clear_all()
                    }
                >
                    "⊘"
                </button>
            </div>
        </div>
    }
}

fn tool_icon(tool: DrawingTool) -> &'static str {
    match tool {
        DrawingTool::Pen => "✎",
        DrawingTool::Arrow => "➚",
        DrawingTool::Rectangle => "▭",
        DrawingTool::Highlight => "▬",
        DrawingTool::Text => "T",
        DrawingTool::Eraser => "⌫",
    }
}
