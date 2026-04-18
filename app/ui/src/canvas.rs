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
    Pen,
    Arrow,
    Rectangle,
    Highlight,
    Text,
    Eraser,
}

impl DrawingTool {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pen => "Pen",
            Self::Arrow => "Arrow",
            Self::Rectangle => "Rectangle",
            Self::Highlight => "Highlight",
            Self::Text => "Text",
            Self::Eraser => "Eraser",
        }
    }

    pub fn cursor(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Eraser => "cell",
            _ => "crosshair",
        }
    }

    pub fn all() -> [DrawingTool; 6] {
        [
            Self::Pen,
            Self::Arrow,
            Self::Rectangle,
            Self::Highlight,
            Self::Text,
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
    Arrow {
        from: (f64, f64),
        to: (f64, f64),
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
            Self::Arrow { from, to, color, width } => json!({
                "type": "arrow",
                "from": [from.0, from.1],
                "to": [to.0, to.1],
                "color": color,
                "width": width,
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
                ctrl.text_input.set(Some(TextInputState {
                    client_x: ev.client_x() as f64,
                    client_y: ev.client_y() as f64,
                    ratio_x: rx,
                    ratio_y: ry,
                }));
                text_input_value.set(String::new());
                return;
            }

            pointer.update_value(|state| {
                state.is_down = true;
                state.start = (rx, ry);
                state.current = vec![(rx, ry)];
                state.preview = None;
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

            let mut preview: Option<Shape> = None;
            pointer.update_value(|state| {
                state.current.push((rx, ry));
                preview = match tool {
                    DrawingTool::Pen => Some(Shape::Path {
                        points: state.current.clone(),
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

    ctx.set_fill_style_str("#ffffff");
    ctx.fill_rect(0.0, 0.0, w, h);

    for shape in ctrl.shapes.get_untracked().iter() {
        render_shape(&ctx, shape, w, h);
    }
    if let Some(shape) = preview {
        render_shape(&ctx, shape, w, h);
    }
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
        Shape::Arrow { from, to, .. } => {
            ((rx - from.0).abs() < r && (ry - from.1).abs() < r)
                || ((rx - to.0).abs() < r && (ry - to.1).abs() < r)
        }
        Shape::Rect { x, y, w, h, .. }
        | Shape::Highlight { x, y, w, h, .. }
        | Shape::Image { x, y, w, h, .. } => {
            rx >= *x - r && rx <= *x + *w + r && ry >= *y - r && ry <= *y + *h + r
        }
        Shape::Text { x, y, .. } => (rx - x).abs() < 0.05 && (ry - y).abs() < 0.05,
    }
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
            }
        }) as Box<dyn FnMut(Event)>);
        let _ = win.add_event_listener_with_callback("keydown", cb.as_ref().unchecked_ref());
        cb.forget();
    });
}
