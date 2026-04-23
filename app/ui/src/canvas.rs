use std::cell::RefCell;
use std::rc::Rc;

use leptos::html::Canvas;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::json;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    window, CanvasRenderingContext2d, ClipboardEvent, DomRect, DragEvent, Event,
    HtmlCanvasElement, HtmlImageElement, HtmlInputElement, KeyboardEvent, MouseEvent,
    ResizeObserver,
};

// ---------------------------------------------------------------------------
// Shared tool state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawingTool {
    Select,
    Pen,
    Line,
    Arrow,
    Rectangle,
    Ellipse,
    Highlight,
    Text,
    Crop,
    Eraser,
}

impl DrawingTool {
    pub fn label(self) -> &'static str {
        match self {
            Self::Select => "Select / Move",
            Self::Pen => "Pen",
            Self::Line => "Line",
            Self::Arrow => "Arrow",
            Self::Rectangle => "Rectangle",
            Self::Ellipse => "Ellipse",
            Self::Highlight => "Highlight",
            Self::Text => "Text",
            Self::Crop => "Crop image",
            Self::Eraser => "Eraser",
        }
    }

    pub fn shortcut(self) -> &'static str {
        match self {
            Self::Select => "V",
            Self::Pen => "P",
            Self::Line => "L",
            Self::Arrow => "A",
            Self::Rectangle => "R",
            Self::Ellipse => "O",
            Self::Highlight => "H",
            Self::Text => "T",
            Self::Crop => "C",
            Self::Eraser => "E",
        }
    }

    pub fn cursor(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Eraser => "cell",
            Self::Select => "default",
            Self::Crop => "crosshair",
            _ => "crosshair",
        }
    }

    pub fn all() -> [DrawingTool; 10] {
        [
            Self::Select,
            Self::Pen,
            Self::Line,
            Self::Arrow,
            Self::Rectangle,
            Self::Ellipse,
            Self::Highlight,
            Self::Text,
            Self::Crop,
            Self::Eraser,
        ]
    }
}

/// Annotation stored in normalised ratio coordinates (0..1) so the canvas can
/// be re-rendered at any size without distorting strokes.
#[derive(Debug, Clone)]
pub enum Shape {
    Path {
        points: Vec<(f64, f64)>,
        color: String,
        width: f64,
    },
    Line {
        from: (f64, f64),
        to: (f64, f64),
        color: String,
        width: f64,
    },
    Arrow {
        from: (f64, f64),
        to: (f64, f64),
        color: String,
        width: f64,
    },
    Ellipse {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        color: String,
        width: f64,
    },
    Rect {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        color: String,
        width: f64,
    },
    Highlight {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        color: String,
    },
    Text {
        x: f64,
        y: f64,
        text: String,
        color: String,
        font_size: f64,
    },
    Image {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        data_url: String,
    },
}

impl Shape {
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Self::Path { points, color, width } => json!({
                "type": "path",
                "points": points.iter().map(|(x, y)| json!([x, y])).collect::<Vec<_>>(),
                "color": color,
                "width": width,
            }),
            Self::Line { from, to, color, width } => json!({
                "type": "line",
                "from": [from.0, from.1],
                "to": [to.0, to.1],
                "color": color,
                "width": width,
            }),
            Self::Arrow { from, to, color, width } => json!({
                "type": "arrow",
                "from": [from.0, from.1],
                "to": [to.0, to.1],
                "color": color,
                "width": width,
            }),
            Self::Ellipse { x, y, w, h, color, width } => json!({
                "type": "ellipse", "x": x, "y": y, "w": w, "h": h, "color": color, "width": width,
            }),
            Self::Rect { x, y, w, h, color, width } => json!({
                "type": "rect", "x": x, "y": y, "w": w, "h": h, "color": color, "width": width,
            }),
            Self::Highlight { x, y, w, h, color } => json!({
                "type": "highlight", "x": x, "y": y, "w": w, "h": h, "color": color,
            }),
            Self::Text { x, y, text, color, font_size } => json!({
                "type": "text", "x": x, "y": y, "text": text, "color": color, "fontSize": font_size,
            }),
            Self::Image { x, y, w, h, .. } => json!({
                "type": "image", "x": x, "y": y, "w": w, "h": h,
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// CanvasController — single source of truth shared between the surface and
// the toolbar / panel via Leptos context. Drawing state lives here so the
// view components stay dumb.
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct CanvasController {
    pub active_tool: RwSignal<DrawingTool>,
    pub stroke_color: RwSignal<String>,
    pub stroke_width: RwSignal<f64>,
    pub shapes: RwSignal<Vec<Shape>>,
    pub undo_stack: RwSignal<Vec<Vec<Shape>>>,
    pub text_input: RwSignal<Option<TextInputState>>,
    pub pasted_base64: RwSignal<Vec<String>>,
    pub hint_dismissed: RwSignal<bool>,
    pub selected_idx: RwSignal<Option<usize>>,
    canvas_node: StoredValue<Option<HtmlCanvasElement>>,
    redraw_trigger: RwSignal<u64>,
}

#[derive(Clone, Debug)]
pub struct TextInputState {
    pub client_x: f64,
    pub client_y: f64,
    pub ratio_x: f64,
    pub ratio_y: f64,
}

impl CanvasController {
    pub fn new() -> Self {
        Self {
            active_tool: RwSignal::new(DrawingTool::Pen),
            stroke_color: RwSignal::new("#ef4444".into()),
            stroke_width: RwSignal::new(3.0),
            shapes: RwSignal::new(Vec::new()),
            undo_stack: RwSignal::new(Vec::new()),
            text_input: RwSignal::new(None),
            pasted_base64: RwSignal::new(Vec::new()),
            hint_dismissed: RwSignal::new(false),
            selected_idx: RwSignal::new(None),
            canvas_node: StoredValue::new(None),
            redraw_trigger: RwSignal::new(0),
        }
    }

    pub fn set_canvas(&self, canvas: HtmlCanvasElement) {
        self.canvas_node.set_value(Some(canvas));
    }

    pub fn add_shape(&self, shape: Shape) {
        let prev = self.shapes.get_untracked();
        let mut next = prev.clone();
        next.push(shape);
        self.shapes.set(next);
        let mut stack = self.undo_stack.get_untracked();
        stack.push(prev);
        if stack.len() > 100 {
            stack.drain(..stack.len() - 100);
        }
        self.undo_stack.set(stack);
        self.poke_redraw();
    }

    pub fn undo(&self) {
        let mut stack = self.undo_stack.get_untracked();
        if let Some(prev) = stack.pop() {
            self.shapes.set(prev);
            self.undo_stack.set(stack);
            self.poke_redraw();
        }
    }

    pub fn clear_all(&self) {
        let prev = self.shapes.get_untracked();
        if !prev.is_empty() {
            let mut stack = self.undo_stack.get_untracked();
            stack.push(prev);
            self.undo_stack.set(stack);
        }
        self.shapes.set(Vec::new());
        self.pasted_base64.set(Vec::new());
        self.poke_redraw();
    }

    pub fn poke_redraw(&self) {
        self.redraw_trigger
            .update(|v| *v = v.wrapping_add(1));
    }

    pub fn redraw_token(&self) -> RwSignal<u64> {
        self.redraw_trigger
    }

    /// Render the canvas to a PNG data URL (without annotations already baked
    /// into the raster). Annotations are stored as vectors; we ask the canvas
    /// for its current pixel data, which *does* include drawn strokes because
    /// the canvas was the live render target.
    pub fn to_png_data_url(&self) -> Option<String> {
        self.canvas_node
            .with_value(|c| c.as_ref().and_then(|c| c.to_data_url_with_type("image/png").ok()))
    }

    pub fn annotations_json(&self) -> Vec<serde_json::Value> {
        self.shapes
            .get_untracked()
            .iter()
            .map(Shape::to_json)
            .collect()
    }

    pub fn cycle_stroke(&self, dir: i32) {
        let sizes = [1.5_f64, 3.0, 6.0];
        let current = self.stroke_width.get_untracked();
        let idx = sizes
            .iter()
            .position(|w| (*w - current).abs() < 0.01)
            .unwrap_or(1);
        let next = ((idx as i32 + dir).rem_euclid(sizes.len() as i32)) as usize;
        self.stroke_width.set(sizes[next]);
    }

    pub fn delete_selected(&self) {
        let Some(idx) = self.selected_idx.get_untracked() else { return };
        let prev = self.shapes.get_untracked();
        if idx >= prev.len() {
            self.selected_idx.set(None);
            return;
        }
        let mut next = prev.clone();
        next.remove(idx);
        let mut stack = self.undo_stack.get_untracked();
        stack.push(prev);
        self.undo_stack.set(stack);
        self.shapes.set(next);
        self.selected_idx.set(None);
        self.poke_redraw();
    }

    pub fn replace_shape(&self, idx: usize, shape: Shape, push_undo: bool) {
        let prev = self.shapes.get_untracked();
        if idx >= prev.len() {
            return;
        }
        let mut next = prev.clone();
        next[idx] = shape;
        if push_undo {
            let mut stack = self.undo_stack.get_untracked();
            stack.push(prev);
            self.undo_stack.set(stack);
        }
        self.shapes.set(next);
        self.poke_redraw();
    }

    pub fn add_pasted_base64(&self, value: String) {
        let mut current = self.pasted_base64.get_untracked();
        current.push(value);
        self.pasted_base64.set(current);
    }
}

impl Default for CanvasController {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Canvas surface component
// ---------------------------------------------------------------------------

#[derive(Default)]
struct PointerState {
    is_down: bool,
    start: (f64, f64),
    current: Vec<(f64, f64)>,
    preview: Option<Shape>,
    /// For Select tool: when dragging an already-selected shape, this records
    /// the shape prior to translation so undo can restore it in one step.
    drag_original: Option<(usize, Shape)>,
    /// For Crop tool: the selected image index and its current ratio bounds.
    crop_target: Option<usize>,
}

#[component]
pub fn CanvasSurface(controller: CanvasController) -> impl IntoView {
    let canvas_node: NodeRef<Canvas> = NodeRef::new();
    let pointer: StoredValue<PointerState> = StoredValue::new(PointerState::default());
    let text_input_value: RwSignal<String> = RwSignal::new(String::new());

    // Keep controller in sync with the mounted canvas element. Fit immediately,
    // then once more next frame (layout may not be final on the very first mount),
    // and install a ResizeObserver so the buffer always matches the rendered box.
    {
        let ctrl = controller.clone();
        Effect::new(move |_| {
            if let Some(el) = canvas_node.get() {
                let canvas: HtmlCanvasElement = el.into();
                ctrl.set_canvas(canvas.clone());
                fit_canvas_to_viewport(&canvas);
                render_all(&canvas, &ctrl);
                schedule_rerender_next_frame(canvas.clone(), ctrl.clone());
                install_resize_observer(canvas, ctrl.clone());
            }
        });
    }

    // Redraw on any shape / color / stroke change.
    {
        let ctrl = controller.clone();
        let token = ctrl.redraw_token();
        Effect::new(move |_| {
            // subscribe
            let _ = token.get();
            let _ = ctrl.shapes.get();
            let _ = ctrl.stroke_color.get();
            let _ = ctrl.stroke_width.get();
            if let Some(el) = canvas_node.get() {
                let canvas: HtmlCanvasElement = el.into();
                render_all(&canvas, &ctrl);
            }
        });
    }

    // Handle window resize — keep canvas full-bleed.
    {
        let ctrl = controller.clone();
        Effect::new(move |already_ran: Option<()>| {
            if already_ran.is_some() {
                return;
            }
            if let Some(win) = window() {
                let ctrl = ctrl.clone();
                let cb = Closure::wrap(Box::new(move |_: Event| {
                    let Some(el) = canvas_node.get() else { return };
                    let canvas: HtmlCanvasElement = el.into();
                    fit_canvas_to_viewport(&canvas);
                    render_all(&canvas, &ctrl);
                }) as Box<dyn FnMut(Event)>);
                let _ = win
                    .add_event_listener_with_callback("resize", cb.as_ref().unchecked_ref());
                cb.forget();
            }
        });
    }

    // Handle paste events on window (screenshots / images).
    install_paste_listener(controller.clone());
    install_keyboard_shortcuts(controller.clone());

    // Pointer handlers.
    let on_mousedown = {
        let ctrl = controller.clone();
        let text_input_value = text_input_value;
        move |ev: MouseEvent| {
            if !ctrl.hint_dismissed.get_untracked() {
                ctrl.hint_dismissed.set(true);
            }
            let tool = ctrl.active_tool.get_untracked();
            let color = ctrl.stroke_color.get_untracked();
            let width = ctrl.stroke_width.get_untracked();
            let Some(el) = canvas_node.get() else { return };
            let canvas: HtmlCanvasElement = el.into();
            fit_canvas_to_viewport(&canvas);
            let rect = canvas.get_bounding_client_rect();
            let (rx, ry) = event_ratio(&ev, &rect);

            if tool == DrawingTool::Text {
                ev.prevent_default();
                // Input is absolutely positioned inside .stage, which does
                // not start at viewport (0,0) — the app bar sits above it.
                // Store stage-relative coords so CSS left/top puts the input
                // where the user clicked rather than offset by the app-bar
                // height.
                ctrl.text_input.set(Some(TextInputState {
                    client_x: ev.client_x() as f64 - rect.left(),
                    client_y: ev.client_y() as f64 - rect.top(),
                    ratio_x: rx,
                    ratio_y: ry,
                }));
                text_input_value.set(String::new());
                return;
            }

            if tool == DrawingTool::Select {
                let picked = pick_shape_at(&ctrl.shapes.get_untracked(), rx, ry);
                ctrl.selected_idx.set(picked);
                pointer.update_value(|state| {
                    state.is_down = true;
                    state.start = (rx, ry);
                    state.current = vec![(rx, ry)];
                    state.preview = None;
                    state.drag_original = picked
                        .and_then(|i| ctrl.shapes.get_untracked().get(i).cloned().map(|s| (i, s)));
                    state.crop_target = None;
                });
                ctrl.poke_redraw();
                return;
            }

            if tool == DrawingTool::Crop {
                let selected = ctrl.selected_idx.get_untracked();
                let target = selected.filter(|i| {
                    matches!(
                        ctrl.shapes.get_untracked().get(*i),
                        Some(Shape::Image { .. })
                    )
                });
                pointer.update_value(|state| {
                    state.is_down = target.is_some();
                    state.start = (rx, ry);
                    state.current = vec![(rx, ry)];
                    state.preview = None;
                    state.drag_original = None;
                    state.crop_target = target;
                });
                return;
            }

            pointer.update_value(|state| {
                state.is_down = true;
                state.start = (rx, ry);
                state.current = vec![(rx, ry)];
                state.preview = None;
                state.drag_original = None;
                state.crop_target = None;
            });

            if tool == DrawingTool::Eraser {
                erase_at(&ctrl, rx, ry, width);
            }
            let _ = color; // silence unused in this branch
        }
    };

    let on_mousemove = {
        let ctrl = controller.clone();
        move |ev: MouseEvent| {
            let Some(el) = canvas_node.get() else { return };
            let canvas: HtmlCanvasElement = el.into();
            let rect = canvas.get_bounding_client_rect();
            let (rx, ry) = event_ratio(&ev, &rect);
            let tool = ctrl.active_tool.get_untracked();
            let color = ctrl.stroke_color.get_untracked();
            let width = ctrl.stroke_width.get_untracked();
            let is_down = pointer.with_value(|s| s.is_down);
            if !is_down || tool == DrawingTool::Text {
                return;
            }

            if tool == DrawingTool::Eraser {
                erase_at(&ctrl, rx, ry, width);
                return;
            }

            if tool == DrawingTool::Select {
                let (start, orig) = pointer.with_value(|s| (s.start, s.drag_original.clone()));
                if let Some((idx, original)) = orig {
                    let dx = rx - start.0;
                    let dy = ry - start.1;
                    let moved = translate_shape(&original, dx, dy);
                    // Live update without pushing undo; undo is recorded on mouseup.
                    ctrl.replace_shape(idx, moved, false);
                }
                return;
            }

            if tool == DrawingTool::Crop {
                let target = pointer.with_value(|s| s.crop_target);
                let (sx, sy) = pointer.with_value(|s| s.start);
                let mut preview: Option<Shape> = None;
                if target.is_some() {
                    preview = Some(Shape::Rect {
                        x: sx.min(rx),
                        y: sy.min(ry),
                        w: (rx - sx).abs(),
                        h: (ry - sy).abs(),
                        color: "#2563eb".into(),
                        width: 1.5,
                    });
                }
                pointer.update_value(|state| state.preview = preview.clone());
                render_all_with_preview(&canvas, &ctrl, preview.as_ref());
                return;
            }

            let mut preview: Option<Shape> = None;
            pointer.update_value(|state| {
                state.current.push((rx, ry));
                preview = match tool {
                    DrawingTool::Pen => Some(Shape::Path {
                        points: state.current.clone(),
                        color: color.clone(),
                        width,
                    }),
                    DrawingTool::Line => Some(Shape::Line {
                        from: state.start,
                        to: (rx, ry),
                        color: color.clone(),
                        width,
                    }),
                    DrawingTool::Arrow => Some(Shape::Arrow {
                        from: state.start,
                        to: (rx, ry),
                        color: color.clone(),
                        width,
                    }),
                    DrawingTool::Rectangle => Some(Shape::Rect {
                        x: state.start.0.min(rx),
                        y: state.start.1.min(ry),
                        w: (rx - state.start.0).abs(),
                        h: (ry - state.start.1).abs(),
                        color: color.clone(),
                        width,
                    }),
                    DrawingTool::Ellipse => Some(Shape::Ellipse {
                        x: state.start.0.min(rx),
                        y: state.start.1.min(ry),
                        w: (rx - state.start.0).abs(),
                        h: (ry - state.start.1).abs(),
                        color: color.clone(),
                        width,
                    }),
                    DrawingTool::Highlight => Some(Shape::Highlight {
                        x: state.start.0.min(rx),
                        y: state.start.1.min(ry),
                        w: (rx - state.start.0).abs(),
                        h: (ry - state.start.1).abs(),
                        color: color.clone(),
                    }),
                    _ => None,
                };
                state.preview = preview.clone();
            });

            render_all_with_preview(&canvas, &ctrl, preview.as_ref());
        }
    };

    let on_mouseup = {
        let ctrl = controller.clone();
        move |ev: MouseEvent| {
            let Some(el) = canvas_node.get() else { return };
            let canvas: HtmlCanvasElement = el.into();
            let rect = canvas.get_bounding_client_rect();
            let (rx, ry) = event_ratio(&ev, &rect);
            let tool = ctrl.active_tool.get_untracked();
            let color = ctrl.stroke_color.get_untracked();
            let width = ctrl.stroke_width.get_untracked();

            if tool == DrawingTool::Select {
                let drag = pointer.with_value(|s| s.drag_original.clone());
                pointer.update_value(|state| {
                    state.is_down = false;
                    state.drag_original = None;
                    state.preview = None;
                });
                if let Some((idx, original)) = drag {
                    let dx = rx - pointer.with_value(|s| s.start.0);
                    let dy = ry - pointer.with_value(|s| s.start.1);
                    if dx.abs() > 0.0005 || dy.abs() > 0.0005 {
                        // Roll back live move, then replay through add_shape semantics
                        // so undo captures a single pre-move snapshot.
                        let moved = ctrl
                            .shapes
                            .get_untracked()
                            .get(idx)
                            .cloned()
                            .unwrap_or(original.clone());
                        ctrl.replace_shape(idx, original, false);
                        ctrl.replace_shape(idx, moved, true);
                    }
                }
                ctrl.poke_redraw();
                return;
            }

            if tool == DrawingTool::Crop {
                let (target, start, preview) =
                    pointer.with_value(|s| (s.crop_target, s.start, s.preview.clone()));
                pointer.update_value(|state| {
                    state.is_down = false;
                    state.preview = None;
                    state.crop_target = None;
                });
                if let (Some(idx), Some(Shape::Rect { x, y, w: rw, h: rh, .. })) =
                    (target, preview)
                {
                    if rw > 0.01 && rh > 0.01 {
                        let ctrl2 = ctrl.clone();
                        let (sx, sy) = start;
                        let _ = sx;
                        let _ = sy;
                        spawn_local(async move {
                            perform_crop(&ctrl2, idx, x, y, rw, rh).await;
                        });
                    }
                }
                ctrl.poke_redraw();
                return;
            }

            let mut shape: Option<Shape> = None;
            pointer.update_value(|state| {
                if !state.is_down {
                    return;
                }
                state.is_down = false;
                match tool {
                    DrawingTool::Pen => {
                        if state.current.len() > 1 {
                            shape = Some(Shape::Path {
                                points: state.current.clone(),
                                color: color.clone(),
                                width,
                            });
                        }
                    }
                    DrawingTool::Line => {
                        if distance_big_enough(state.start, (rx, ry)) {
                            shape = Some(Shape::Line {
                                from: state.start,
                                to: (rx, ry),
                                color: color.clone(),
                                width,
                            });
                        }
                    }
                    DrawingTool::Arrow => {
                        if distance_big_enough(state.start, (rx, ry)) {
                            shape = Some(Shape::Arrow {
                                from: state.start,
                                to: (rx, ry),
                                color: color.clone(),
                                width,
                            });
                        }
                    }
                    DrawingTool::Rectangle => {
                        if distance_big_enough(state.start, (rx, ry)) {
                            shape = Some(Shape::Rect {
                                x: state.start.0.min(rx),
                                y: state.start.1.min(ry),
                                w: (rx - state.start.0).abs(),
                                h: (ry - state.start.1).abs(),
                                color: color.clone(),
                                width,
                            });
                        }
                    }
                    DrawingTool::Ellipse => {
                        if distance_big_enough(state.start, (rx, ry)) {
                            shape = Some(Shape::Ellipse {
                                x: state.start.0.min(rx),
                                y: state.start.1.min(ry),
                                w: (rx - state.start.0).abs(),
                                h: (ry - state.start.1).abs(),
                                color: color.clone(),
                                width,
                            });
                        }
                    }
                    DrawingTool::Highlight => {
                        if distance_big_enough(state.start, (rx, ry)) {
                            shape = Some(Shape::Highlight {
                                x: state.start.0.min(rx),
                                y: state.start.1.min(ry),
                                w: (rx - state.start.0).abs(),
                                h: (ry - state.start.1).abs(),
                                color: color.clone(),
                            });
                        }
                    }
                    _ => {}
                }
                state.preview = None;
            });
            if let Some(s) = shape {
                ctrl.add_shape(s);
            } else {
                ctrl.poke_redraw();
            }
        }
    };

    // Text input helpers.
    let commit_text = {
        let ctrl = controller.clone();
        let value = text_input_value;
        move || {
            let Some(state) = ctrl.text_input.get_untracked() else { return };
            let text = value.get_untracked().trim().to_string();
            ctrl.text_input.set(None);
            if text.is_empty() {
                return;
            }
            let width = ctrl.stroke_width.get_untracked();
            ctrl.add_shape(Shape::Text {
                x: state.ratio_x,
                y: state.ratio_y,
                text,
                color: ctrl.stroke_color.get_untracked(),
                font_size: 14.0 + width * 2.0,
            });
        }
    };

    let cancel_text = {
        let ctrl = controller.clone();
        let value = text_input_value;
        move || {
            ctrl.text_input.set(None);
            value.set(String::new());
        }
    };

    let on_text_keydown = {
        let commit = commit_text.clone();
        let cancel = cancel_text.clone();
        move |ev: KeyboardEvent| {
            let key = ev.key();
            if key == "Enter" {
                ev.prevent_default();
                commit();
            } else if key == "Escape" {
                cancel();
            }
        }
    };

    // Drag-and-drop image files onto the canvas.
    let on_drop = {
        let ctrl = controller.clone();
        move |ev: DragEvent| {
            ev.prevent_default();
            let Some(dt) = ev.data_transfer() else { return };
            let files = dt.files();
            if let Some(files) = files {
                let ctrl = ctrl.clone();
                for i in 0..files.length() {
                    if let Some(file) = files.get(i) {
                        let ctrl = ctrl.clone();
                        let reader = match web_sys::FileReader::new() {
                            Ok(r) => r,
                            Err(_) => continue,
                        };
                        let reader_clone = reader.clone();
                        let onload = Closure::wrap(Box::new(move |_: Event| {
                            if let Ok(value) = reader_clone.result() {
                                if let Some(data_url) = value.as_string() {
                                    ingest_image_data_url(&ctrl, data_url);
                                }
                            }
                        }) as Box<dyn FnMut(Event)>);
                        reader.set_onload(Some(onload.as_ref().unchecked_ref()));
                        let _ = reader.read_as_data_url(&file);
                        onload.forget();
                    }
                }
            }
        }
    };

    let on_dragover = move |ev: DragEvent| {
        ev.prevent_default();
    };

    view! {
        <div
            class="stage"
            on:drop=on_drop
            on:dragover=on_dragover
        >
            <canvas
                node_ref=canvas_node
                class="canvas-surface"
                style:cursor=move || controller.active_tool.get().cursor()
                on:mousedown=on_mousedown
                on:mousemove=on_mousemove
                on:mouseup=on_mouseup
            ></canvas>

            <EmptyHint controller=controller.clone() />

            {
                let ctrl = controller.clone();
                let commit = commit_text.clone();
                let keydown = on_text_keydown.clone();
                move || {
                    let Some(state) = ctrl.text_input.get() else {
                        return view! { <span></span> }.into_any();
                    };
                    let commit = commit.clone();
                    let keydown = keydown.clone();
                    view! {
                        <input
                            type="text"
                            class="canvas-text-input"
                            style:left=format!("{}px", state.client_x)
                            style:top=format!("{}px", state.client_y)
                            prop:value=move || text_input_value.get()
                            placeholder="Type…"
                            autofocus=true
                            on:input=move |ev| {
                                let target = event_target::<HtmlInputElement>(&ev);
                                text_input_value.set(target.value());
                            }
                            on:blur=move |_| commit()
                            on:keydown=keydown
                        />
                    }
                    .into_any()
                }
            }
        </div>
    }
}

#[component]
fn EmptyHint(controller: CanvasController) -> impl IntoView {
    view! {
        {move || {
            if controller.hint_dismissed.get()
                || !controller.shapes.get().is_empty()
                || !controller.pasted_base64.get().is_empty()
            {
                view! { <span></span> }.into_any()
            } else {
                view! {
                    <div class="canvas-hint">
                        <div><strong>"Draw, type, paste, or speak"</strong></div>
                        <div>"Cmd/Ctrl+V pastes screenshots. Drag images in."</div>
                    </div>
                }
                .into_any()
            }
        }}
    }
}

// ---------------------------------------------------------------------------
// Canvas rendering helpers
// ---------------------------------------------------------------------------

fn fit_canvas_to_viewport(canvas: &HtmlCanvasElement) {
    let (w, h) = canvas_pixel_size(canvas);
    if canvas.width() != w {
        canvas.set_width(w);
    }
    if canvas.height() != h {
        canvas.set_height(h);
    }
}

fn canvas_pixel_size(canvas: &HtmlCanvasElement) -> (u32, u32) {
    let rect = canvas.get_bounding_client_rect();
    let w = rect.width().max(1.0).round() as u32;
    let h = rect.height().max(1.0).round() as u32;
    (w, h)
}

/// After initial mount the layout may not be final on the first tick, so the
/// first `fit_canvas_to_viewport` can snapshot a stale / too-small rect. We
/// re-run once on the next animation frame when the DOM has settled.
fn schedule_rerender_next_frame(canvas: HtmlCanvasElement, ctrl: CanvasController) {
    let Some(win) = window() else { return };
    let cb = Closure::once_into_js(move || {
        fit_canvas_to_viewport(&canvas);
        render_all(&canvas, &ctrl);
    });
    let _ = win.request_animation_frame(cb.as_ref().unchecked_ref());
}

/// Keep the canvas bitmap in sync with its CSS box. Without this the bitmap
/// buffer stays at its mount-time size and drawings get squished into a
/// sub-rectangle of the visible element.
fn install_resize_observer(canvas: HtmlCanvasElement, ctrl: CanvasController) {
    let canvas_for_cb = canvas.clone();
    let cb = Closure::wrap(Box::new(move |_entries: js_sys::Array, _obs: ResizeObserver| {
        fit_canvas_to_viewport(&canvas_for_cb);
        render_all(&canvas_for_cb, &ctrl);
    }) as Box<dyn FnMut(js_sys::Array, ResizeObserver)>);
    if let Ok(observer) = ResizeObserver::new(cb.as_ref().unchecked_ref()) {
        observer.observe(&canvas);
        // Observer must outlive this scope — it tracks this canvas for its
        // entire lifetime, so leaking is intentional.
        std::mem::forget(observer);
    }
    cb.forget();
}

fn render_all(canvas: &HtmlCanvasElement, ctrl: &CanvasController) {
    render_all_with_preview(canvas, ctrl, None);
}

fn render_all_with_preview(
    canvas: &HtmlCanvasElement,
    ctrl: &CanvasController,
    preview: Option<&Shape>,
) {
    // Keep bitmap in sync with displayed CSS box. This is a no-op whenever the
    // size hasn't drifted, but protects against cases where the ResizeObserver
    // hasn't caught up yet (first frame after mount, window resize mid-drag).
    fit_canvas_to_viewport(canvas);
    let Some(ctx) = canvas_context(canvas) else { return };
    let w = canvas.width() as f64;
    let h = canvas.height() as f64;

    ctx.clear_rect(0.0, 0.0, w, h);

    let shapes = ctrl.shapes.get_untracked();
    let selected = ctrl.selected_idx.get_untracked();
    for (i, shape) in shapes.iter().enumerate() {
        render_shape(&ctx, shape, w, h);
        if Some(i) == selected {
            if let Some((bx, by, bw, bh)) = shape_bounds(shape) {
                draw_selection_box(&ctx, bx * w, by * h, bw * w, bh * h);
            }
        }
    }
    if let Some(shape) = preview {
        render_shape(&ctx, shape, w, h);
    }
}

fn draw_selection_box(ctx: &CanvasRenderingContext2d, x: f64, y: f64, w: f64, h: f64) {
    ctx.save();
    ctx.set_stroke_style_str("#2563eb");
    ctx.set_line_width(1.5);
    let dash = js_sys::Array::new();
    dash.push(&wasm_bindgen::JsValue::from_f64(6.0));
    dash.push(&wasm_bindgen::JsValue::from_f64(4.0));
    let _ = ctx.set_line_dash(&dash);
    let pad = 4.0;
    ctx.stroke_rect(x - pad, y - pad, w + pad * 2.0, h + pad * 2.0);
    let empty = js_sys::Array::new();
    let _ = ctx.set_line_dash(&empty);
    ctx.restore();
}

fn canvas_context(canvas: &HtmlCanvasElement) -> Option<CanvasRenderingContext2d> {
    canvas
        .get_context("2d")
        .ok()
        .flatten()
        .and_then(|c| c.dyn_into::<CanvasRenderingContext2d>().ok())
}

fn render_shape(ctx: &CanvasRenderingContext2d, shape: &Shape, w: f64, h: f64) {
    ctx.save();
    match shape {
        Shape::Path { points, color, width } => {
            if points.len() > 1 {
                ctx.set_stroke_style_str(color);
                ctx.set_line_width(*width);
                ctx.set_line_cap("round");
                ctx.set_line_join("round");
                ctx.begin_path();
                let (fx, fy) = points[0];
                ctx.move_to(fx * w, fy * h);
                for pt in points.iter().skip(1) {
                    ctx.line_to(pt.0 * w, pt.1 * h);
                }
                ctx.stroke();
            }
        }
        Shape::Line { from, to, color, width } => {
            ctx.set_stroke_style_str(color);
            ctx.set_line_width(*width);
            ctx.set_line_cap("round");
            ctx.begin_path();
            ctx.move_to(from.0 * w, from.1 * h);
            ctx.line_to(to.0 * w, to.1 * h);
            ctx.stroke();
        }
        Shape::Ellipse { x, y, w: rw, h: rh, color, width } => {
            ctx.set_stroke_style_str(color);
            ctx.set_line_width(*width);
            let cx = (x + rw / 2.0) * w;
            let cy = (y + rh / 2.0) * h;
            let radx = (rw / 2.0) * w;
            let rady = (rh / 2.0) * h;
            ctx.begin_path();
            let _ = ctx.ellipse(
                cx,
                cy,
                radx.max(0.0),
                rady.max(0.0),
                0.0,
                0.0,
                std::f64::consts::TAU,
            );
            ctx.stroke();
        }
        Shape::Arrow { from, to, color, width } => {
            ctx.set_stroke_style_str(color);
            ctx.set_line_width(*width);
            ctx.set_line_cap("round");
            let fx = from.0 * w;
            let fy = from.1 * h;
            let tx = to.0 * w;
            let ty = to.1 * h;
            ctx.begin_path();
            ctx.move_to(fx, fy);
            ctx.line_to(tx, ty);
            ctx.stroke();
            let angle = (ty - fy).atan2(tx - fx);
            let head = *width * 5.0 + 6.0;
            ctx.begin_path();
            ctx.move_to(tx, ty);
            ctx.line_to(
                tx - head * (angle - std::f64::consts::FRAC_PI_6).cos(),
                ty - head * (angle - std::f64::consts::FRAC_PI_6).sin(),
            );
            ctx.move_to(tx, ty);
            ctx.line_to(
                tx - head * (angle + std::f64::consts::FRAC_PI_6).cos(),
                ty - head * (angle + std::f64::consts::FRAC_PI_6).sin(),
            );
            ctx.stroke();
        }
        Shape::Rect { x, y, w: rw, h: rh, color, width } => {
            ctx.set_stroke_style_str(color);
            ctx.set_line_width(*width);
            ctx.stroke_rect(x * w, y * h, rw * w, rh * h);
        }
        Shape::Highlight { x, y, w: rw, h: rh, color } => {
            ctx.set_global_alpha(0.35);
            ctx.set_fill_style_str(color);
            ctx.fill_rect(x * w, y * h, rw * w, rh * h);
        }
        Shape::Text { x, y, text, color, font_size } => {
            ctx.set_fill_style_str(color);
            ctx.set_font(&format!("{font_size}px -apple-system, sans-serif"));
            ctx.set_text_baseline("top");
            let _ = ctx.fill_text(text, x * w, y * h);
        }
        Shape::Image { x, y, w: iw, h: ih, data_url } => {
            if let Ok(img) = HtmlImageElement::new() {
                img.set_src(data_url);
                // If the image is already decoded, draw now; otherwise hook onload.
                if img.complete() && img.natural_width() > 0 {
                    let _ = ctx.draw_image_with_html_image_element_and_dw_and_dh(
                        &img,
                        x * w,
                        y * h,
                        iw * w,
                        ih * h,
                    );
                } else {
                    let ctx2 = ctx.clone();
                    let img_clone = img.clone();
                    let (x, y, iw, ih) = (*x, *y, *iw, *ih);
                    let onload = Closure::wrap(Box::new(move |_: Event| {
                        let _ = ctx2.draw_image_with_html_image_element_and_dw_and_dh(
                            &img_clone,
                            x * w,
                            y * h,
                            iw * w,
                            ih * h,
                        );
                    }) as Box<dyn FnMut(Event)>);
                    img.set_onload(Some(onload.as_ref().unchecked_ref()));
                    onload.forget();
                }
            }
        }
    }
    ctx.restore();
}

fn distance_big_enough(a: (f64, f64), b: (f64, f64)) -> bool {
    (a.0 - b.0).abs() > 0.005 || (a.1 - b.1).abs() > 0.005
}

fn event_ratio(ev: &MouseEvent, rect: &DomRect) -> (f64, f64) {
    let w = rect.width().max(1.0);
    let h = rect.height().max(1.0);
    let x = (ev.client_x() as f64 - rect.left()) / w;
    let y = (ev.client_y() as f64 - rect.top()) / h;
    (x.clamp(0.0, 1.0), y.clamp(0.0, 1.0))
}

fn erase_at(ctrl: &CanvasController, rx: f64, ry: f64, width: f64) {
    let radius_r = 0.02 * (width.max(1.0) / 2.0);
    let shapes = ctrl.shapes.get_untracked();
    let filtered: Vec<Shape> = shapes
        .into_iter()
        .filter(|s| !shape_hit(s, rx, ry, radius_r))
        .collect();
    if filtered.len() != ctrl.shapes.get_untracked().len() {
        ctrl.shapes.set(filtered);
        ctrl.poke_redraw();
    }
}

fn shape_hit(shape: &Shape, rx: f64, ry: f64, r: f64) -> bool {
    match shape {
        Shape::Path { points, .. } => points
            .iter()
            .any(|(px, py)| (px - rx).abs() < r && (py - ry).abs() < r),
        Shape::Line { from, to, .. } | Shape::Arrow { from, to, .. } => {
            ((rx - from.0).abs() < r && (ry - from.1).abs() < r)
                || ((rx - to.0).abs() < r && (ry - to.1).abs() < r)
        }
        Shape::Rect { x, y, w, h, .. }
        | Shape::Ellipse { x, y, w, h, .. }
        | Shape::Highlight { x, y, w, h, .. }
        | Shape::Image { x, y, w, h, .. } => {
            rx >= *x - r && rx <= *x + *w + r && ry >= *y - r && ry <= *y + *h + r
        }
        Shape::Text { x, y, .. } => (rx - x).abs() < 0.05 && (ry - y).abs() < 0.05,
    }
}

/// Axis-aligned bounding box in ratio coords, for selection/picking.
fn shape_bounds(shape: &Shape) -> Option<(f64, f64, f64, f64)> {
    match shape {
        Shape::Path { points, .. } => {
            if points.is_empty() {
                return None;
            }
            let (mut minx, mut miny) = (f64::INFINITY, f64::INFINITY);
            let (mut maxx, mut maxy) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
            for (px, py) in points {
                minx = minx.min(*px);
                miny = miny.min(*py);
                maxx = maxx.max(*px);
                maxy = maxy.max(*py);
            }
            Some((minx, miny, (maxx - minx).max(0.0), (maxy - miny).max(0.0)))
        }
        Shape::Line { from, to, .. } | Shape::Arrow { from, to, .. } => {
            let x = from.0.min(to.0);
            let y = from.1.min(to.1);
            Some((x, y, (from.0 - to.0).abs(), (from.1 - to.1).abs()))
        }
        Shape::Rect { x, y, w, h, .. }
        | Shape::Ellipse { x, y, w, h, .. }
        | Shape::Highlight { x, y, w, h, .. }
        | Shape::Image { x, y, w, h, .. } => Some((*x, *y, *w, *h)),
        Shape::Text { x, y, .. } => Some((*x, *y, 0.1, 0.04)),
    }
}

/// Hit-test shapes under a pointer. Searches top-most first so the visually
/// front-most shape wins. Used by the Select tool.
fn pick_shape_at(shapes: &[Shape], rx: f64, ry: f64) -> Option<usize> {
    for (i, shape) in shapes.iter().enumerate().rev() {
        if let Some((x, y, w, h)) = shape_bounds(shape) {
            let pad = 0.005;
            if rx >= x - pad && rx <= x + w + pad && ry >= y - pad && ry <= y + h + pad {
                return Some(i);
            }
        }
    }
    None
}

fn translate_shape(shape: &Shape, dx: f64, dy: f64) -> Shape {
    let t = |(a, b): (f64, f64)| (a + dx, b + dy);
    match shape {
        Shape::Path { points, color, width } => Shape::Path {
            points: points.iter().map(|p| t(*p)).collect(),
            color: color.clone(),
            width: *width,
        },
        Shape::Line { from, to, color, width } => Shape::Line {
            from: t(*from),
            to: t(*to),
            color: color.clone(),
            width: *width,
        },
        Shape::Arrow { from, to, color, width } => Shape::Arrow {
            from: t(*from),
            to: t(*to),
            color: color.clone(),
            width: *width,
        },
        Shape::Rect { x, y, w, h, color, width } => Shape::Rect {
            x: x + dx,
            y: y + dy,
            w: *w,
            h: *h,
            color: color.clone(),
            width: *width,
        },
        Shape::Ellipse { x, y, w, h, color, width } => Shape::Ellipse {
            x: x + dx,
            y: y + dy,
            w: *w,
            h: *h,
            color: color.clone(),
            width: *width,
        },
        Shape::Highlight { x, y, w, h, color } => Shape::Highlight {
            x: x + dx,
            y: y + dy,
            w: *w,
            h: *h,
            color: color.clone(),
        },
        Shape::Text { x, y, text, color, font_size } => Shape::Text {
            x: x + dx,
            y: y + dy,
            text: text.clone(),
            color: color.clone(),
            font_size: *font_size,
        },
        Shape::Image { x, y, w, h, data_url } => Shape::Image {
            x: x + dx,
            y: y + dy,
            w: *w,
            h: *h,
            data_url: data_url.clone(),
        },
    }
}

/// Crop the Image at `idx` to the canvas-ratio rect (cx, cy, cw, ch),
/// clamped to the image's own bounds. Produces a new data URL via an
/// offscreen canvas and replaces the shape in place, recording an undo entry.
async fn perform_crop(
    ctrl: &CanvasController,
    idx: usize,
    cx: f64,
    cy: f64,
    cw: f64,
    ch: f64,
) {
    let shapes = ctrl.shapes.get_untracked();
    let Some(Shape::Image { x: ix, y: iy, w: iw, h: ih, data_url }) = shapes.get(idx).cloned()
    else {
        return;
    };
    // Intersect crop rect with image bounds in ratio space.
    let rx0 = cx.max(ix);
    let ry0 = cy.max(iy);
    let rx1 = (cx + cw).min(ix + iw);
    let ry1 = (cy + ch).min(iy + ih);
    if rx1 <= rx0 || ry1 <= ry0 {
        return;
    }
    // Map crop rect into source-image fraction coords.
    let fx = (rx0 - ix) / iw;
    let fy = (ry0 - iy) / ih;
    let fw = (rx1 - rx0) / iw;
    let fh = (ry1 - ry0) / ih;

    let img = match HtmlImageElement::new() {
        Ok(i) => i,
        Err(_) => return,
    };
    img.set_src(&data_url);
    let _ = JsFuture::from(img.decode()).await;
    let nw = img.natural_width() as f64;
    let nh = img.natural_height() as f64;
    if nw <= 0.0 || nh <= 0.0 {
        return;
    }
    let sx = fx * nw;
    let sy = fy * nh;
    let sw = fw * nw;
    let sh = fh * nh;

    let Some(doc) = window().and_then(|w| w.document()) else { return };
    let off: HtmlCanvasElement = match doc.create_element("canvas").and_then(|e| {
        e.dyn_into::<HtmlCanvasElement>()
            .map_err(|_| wasm_bindgen::JsValue::NULL.into())
    }) {
        Ok(c) => c,
        Err(_) => return,
    };
    off.set_width(sw.max(1.0) as u32);
    off.set_height(sh.max(1.0) as u32);
    let Some(ctx) = canvas_context(&off) else { return };
    if ctx
        .draw_image_with_html_image_element_and_sw_and_sh_and_dx_and_dy_and_dw_and_dh(
            &img,
            sx,
            sy,
            sw,
            sh,
            0.0,
            0.0,
            sw,
            sh,
        )
        .is_err()
    {
        return;
    }
    let Ok(new_url) = off.to_data_url_with_type("image/png") else { return };

    let new_shape = Shape::Image {
        x: rx0,
        y: ry0,
        w: rx1 - rx0,
        h: ry1 - ry0,
        data_url: new_url,
    };
    ctrl.replace_shape(idx, new_shape, true);
}

// ---------------------------------------------------------------------------
// Image ingestion (paste + drop)
// ---------------------------------------------------------------------------

pub fn ingest_image_data_url(ctrl: &CanvasController, data_url: String) {
    if !data_url.starts_with("data:image/") {
        return;
    }
    ctrl.hint_dismissed.set(true);
    if let Some(b64) = data_url.split(',').nth(1) {
        ctrl.add_pasted_base64(b64.to_string());
    }
    // Drop the image at its natural pixel size, centred. If it's larger than
    // the canvas we scale down to fit (keeping aspect ratio) so it stays
    // visible; otherwise we render it 1:1.
    let ctrl2 = ctrl.clone();
    let data_for_resolve = data_url.clone();
    spawn_local(async move {
        let canvas_px = ctrl2
            .canvas_node
            .with_value(|c| c.as_ref().map(canvas_pixel_size));
        let (cw, ch) = match canvas_px {
            Some((w, h)) if w > 0 && h > 0 => (w as f64, h as f64),
            _ => {
                ctrl2.add_shape(Shape::Image {
                    x: 0.3, y: 0.3, w: 0.4, h: 0.4, data_url: data_for_resolve,
                });
                return;
            }
        };
        if let Some((nw, nh)) = decode_image_size(&data_for_resolve).await {
            let scale = (cw / nw).min(ch / nh).min(1.0);
            let iw = (nw * scale) / cw;
            let ih = (nh * scale) / ch;
            let x = 0.5 - iw / 2.0;
            let y = 0.5 - ih / 2.0;
            ctrl2.add_shape(Shape::Image {
                x, y, w: iw, h: ih, data_url: data_for_resolve,
            });
        } else {
            ctrl2.add_shape(Shape::Image {
                x: 0.3, y: 0.3, w: 0.4, h: 0.4, data_url: data_for_resolve,
            });
        }
    });
}

async fn decode_image_size(data_url: &str) -> Option<(f64, f64)> {
    let img = HtmlImageElement::new().ok()?;
    img.set_src(data_url);
    let decode = img.decode();
    let _ = JsFuture::from(decode).await;
    let nw = img.natural_width() as f64;
    let nh = img.natural_height() as f64;
    if nw <= 0.0 || nh <= 0.0 {
        return None;
    }
    Some((nw, nh))
}

// ---------------------------------------------------------------------------
// Paste + keyboard wiring (global listeners on window)
// ---------------------------------------------------------------------------

fn install_paste_listener(ctrl: CanvasController) {
    Effect::new(move |already_ran: Option<()>| {
        if already_ran.is_some() {
            return;
        }
        let Some(win) = window() else { return };
        let ctrl = Rc::new(RefCell::new(ctrl.clone()));
        let cb = Closure::wrap(Box::new(move |ev: Event| {
            let Some(ev) = ev.dyn_ref::<ClipboardEvent>() else { return };
            let Some(data) = ev.clipboard_data() else { return };
            let Some(items) = data.items().dyn_ref::<web_sys::DataTransferItemList>().cloned() else {
                return;
            };
            let mut handled = false;
            for i in 0..items.length() {
                let Some(item) = items.get(i) else { continue };
                if item.type_().starts_with("image/") {
                    if let Ok(Some(blob)) = item.get_as_file() {
                        handled = true;
                        let reader = match web_sys::FileReader::new() {
                            Ok(r) => r,
                            Err(_) => continue,
                        };
                        let reader_clone = reader.clone();
                        let ctrl = ctrl.clone();
                        let onload = Closure::wrap(Box::new(move |_: Event| {
                            if let Ok(val) = reader_clone.result() {
                                if let Some(data_url) = val.as_string() {
                                    ingest_image_data_url(&ctrl.borrow(), data_url);
                                }
                            }
                        }) as Box<dyn FnMut(Event)>);
                        reader.set_onload(Some(onload.as_ref().unchecked_ref()));
                        let _ = reader.read_as_data_url(&blob);
                        onload.forget();
                    }
                }
            }
            if handled {
                ev.prevent_default();
            }
        }) as Box<dyn FnMut(Event)>);
        let _ = win.add_event_listener_with_callback("paste", cb.as_ref().unchecked_ref());
        cb.forget();
    });
}

fn install_keyboard_shortcuts(ctrl: CanvasController) {
    Effect::new(move |already_ran: Option<()>| {
        if already_ran.is_some() {
            return;
        }
        let Some(win) = window() else { return };
        let ctrl = ctrl.clone();
        let cb = Closure::wrap(Box::new(move |ev: Event| {
            let Some(kev) = ev.dyn_ref::<KeyboardEvent>() else { return };
            let cmd_or_ctrl = kev.meta_key() || kev.ctrl_key();
            if cmd_or_ctrl && kev.key() == "z" && !kev.shift_key() {
                ctrl.undo();
                kev.prevent_default();
                return;
            }
            // Skip single-letter tool shortcuts when the user is typing in an
            // input / textarea / contenteditable — avoids hijacking feedback
            // text entry or the canvas text-input box.
            if is_editable_target(kev) {
                return;
            }
            if kev.meta_key() || kev.ctrl_key() || kev.alt_key() {
                return;
            }
            let key = kev.key();
            let tool = match key.as_str() {
                "v" | "V" => Some(DrawingTool::Select),
                "p" | "P" => Some(DrawingTool::Pen),
                "l" | "L" => Some(DrawingTool::Line),
                "a" | "A" => Some(DrawingTool::Arrow),
                "r" | "R" => Some(DrawingTool::Rectangle),
                "o" | "O" => Some(DrawingTool::Ellipse),
                "h" | "H" => Some(DrawingTool::Highlight),
                "t" | "T" => Some(DrawingTool::Text),
                "c" | "C" => Some(DrawingTool::Crop),
                "e" | "E" => Some(DrawingTool::Eraser),
                _ => None,
            };
            if let Some(t) = tool {
                ctrl.active_tool.set(t);
                kev.prevent_default();
                return;
            }
            match key.as_str() {
                "[" => {
                    ctrl.cycle_stroke(-1);
                    kev.prevent_default();
                }
                "]" => {
                    ctrl.cycle_stroke(1);
                    kev.prevent_default();
                }
                "Backspace" | "Delete" => {
                    if ctrl.selected_idx.get_untracked().is_some() {
                        ctrl.delete_selected();
                        kev.prevent_default();
                    }
                }
                "Escape" => {
                    ctrl.selected_idx.set(None);
                    ctrl.poke_redraw();
                }
                _ => {}
            }
        }) as Box<dyn FnMut(Event)>);
        let _ = win.add_event_listener_with_callback("keydown", cb.as_ref().unchecked_ref());
        cb.forget();
    });
}

fn is_editable_target(ev: &KeyboardEvent) -> bool {
    let Some(target) = ev.target() else { return false };
    let Some(el) = target.dyn_ref::<web_sys::Element>() else { return false };
    let tag = el.tag_name().to_lowercase();
    if tag == "input" || tag == "textarea" {
        return true;
    }
    if let Some(html) = el.dyn_ref::<web_sys::HtmlElement>() {
        if html.is_content_editable() {
            return true;
        }
    }
    false
}
