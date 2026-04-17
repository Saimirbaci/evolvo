use leptos::prelude::*;

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
    view! {
        <div class="toolbar" aria-label="Drawing tools">
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
