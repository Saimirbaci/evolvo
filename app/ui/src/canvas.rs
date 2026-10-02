use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use leptos::html::{Canvas, Input};
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::json;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    window, CanvasRenderingContext2d, ClipboardEvent, DomRect, DragEvent, Event, HtmlCanvasElement,
    HtmlImageElement, HtmlInputElement, KeyboardEvent, MouseEvent, ResizeObserver,
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
            Self::Select => "Select / Edit",
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
#[derive(Debug, Clone, PartialEq)]
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
        /// Shared so undo/redo snapshots don't copy multi-MB image bytes.
        data_url: Arc<str>,
    },
}

impl Shape {
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Self::Path {
                points,
                color,
                width,
            } => json!({
                "type": "path",
                "points": points.iter().map(|(x, y)| json!([x, y])).collect::<Vec<_>>(),
                "color": color,
                "width": width,
            }),
            Self::Line {
                from,
                to,
                color,
                width,
            } => json!({
                "type": "line",
                "from": [from.0, from.1],
                "to": [to.0, to.1],
                "color": color,
                "width": width,
            }),
            Self::Arrow {
                from,
                to,
                color,
                width,
            } => json!({
                "type": "arrow",
                "from": [from.0, from.1],
                "to": [to.0, to.1],
                "color": color,
                "width": width,
            }),
            Self::Ellipse {
                x,
                y,
                w,
                h,
                color,
                width,
            } => json!({
                "type": "ellipse", "x": x, "y": y, "w": w, "h": h, "color": color, "width": width,
            }),
            Self::Rect {
                x,
                y,
                w,
                h,
                color,
                width,
            } => json!({
                "type": "rect", "x": x, "y": y, "w": w, "h": h, "color": color, "width": width,
            }),
            Self::Highlight { x, y, w, h, color } => json!({
                "type": "highlight", "x": x, "y": y, "w": w, "h": h, "color": color,
            }),
            Self::Text {
                x,
                y,
                text,
                color,
                font_size,
            } => json!({
                "type": "text", "x": x, "y": y, "text": text, "color": color, "fontSize": font_size,
            }),
            Self::Image { x, y, w, h, .. } => json!({
                "type": "image", "x": x, "y": y, "w": w, "h": h,
            }),
        }
    }

    /// Reconstruct a vector annotation saved by `to_json`. Image bytes are
    /// intentionally supplied out-of-band because persisted annotation JSON
    /// contains geometry and an asset index, never a multi-megabyte data URL.
    pub fn from_json(value: &serde_json::Value, image_data_url: Option<&str>) -> Option<Self> {
        let kind = value.get("type")?.as_str()?;
        let number = |key: &str| value.get(key).and_then(serde_json::Value::as_f64);
        let text = |key: &str| value.get(key).and_then(serde_json::Value::as_str);
        let point = |key: &str| {
            let values = value.get(key)?.as_array()?;
            Some((values.first()?.as_f64()?, values.get(1)?.as_f64()?))
        };
        let color = || text("color").unwrap_or("#ef4444").to_string();
        let width = || number("width").unwrap_or(3.0);

        match kind {
            "path" => Some(Self::Path {
                points: value
                    .get("points")?
                    .as_array()?
                    .iter()
                    .filter_map(|point| {
                        let pair = point.as_array()?;
                        Some((pair.first()?.as_f64()?, pair.get(1)?.as_f64()?))
                    })
                    .collect(),
                color: color(),
                width: width(),
            }),
            "line" => Some(Self::Line {
                from: point("from")?,
                to: point("to")?,
                color: color(),
                width: width(),
            }),
            "arrow" => Some(Self::Arrow {
                from: point("from")?,
                to: point("to")?,
                color: color(),
                width: width(),
            }),
            "ellipse" => Some(Self::Ellipse {
                x: number("x")?,
                y: number("y")?,
                w: number("w")?,
                h: number("h")?,
                color: color(),
                width: width(),
            }),
            "rect" => Some(Self::Rect {
                x: number("x")?,
                y: number("y")?,
                w: number("w")?,
                h: number("h")?,
                color: color(),
                width: width(),
            }),
            "highlight" => Some(Self::Highlight {
                x: number("x")?,
                y: number("y")?,
                w: number("w")?,
                h: number("h")?,
                color: color(),
            }),
            "text" => Some(Self::Text {
                x: number("x")?,
                y: number("y")?,
                text: text("text")?.to_string(),
                color: color(),
                font_size: number("fontSize").unwrap_or(16.0),
            }),
            "image" => Some(Self::Image {
                x: number("x")?,
                y: number("y")?,
                w: number("w")?,
                h: number("h")?,
                data_url: image_data_url?.into(),
            }),
            _ => None,
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
    pub history: RwSignal<History>,
    pub text_input: RwSignal<Option<TextInputState>>,
    pub pasted_base64: RwSignal<Vec<String>>,
    pub hint_dismissed: RwSignal<bool>,
    pub selected_idx: RwSignal<Option<usize>>,
    canvas_node: StoredValue<Option<HtmlCanvasElement>>,
    redraw_trigger: RwSignal<u64>,
    /// State before an in-progress drag (move / resize / erase). Live edits
    /// during the drag skip history; `end_gesture` records one entry.
    gesture_base: StoredValue<Option<Snapshot>>,
}

/// One undo/redo entry: the full shape list plus which shape was selected,
/// so undo also restores what the user was working on.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    shapes: Vec<Shape>,
    selected: Option<usize>,
}

/// Linear undo/redo history. Any new edit clears the redo branch.
#[derive(Debug, Clone, Default)]
pub struct History {
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
}

/// Max undo entries kept per canvas (per storyboard screen).
const HISTORY_LIMIT: usize = 100;

impl History {
    fn record(&mut self, before: Snapshot) {
        self.undo.push(before);
        if self.undo.len() > HISTORY_LIMIT {
            self.undo.drain(..self.undo.len() - HISTORY_LIMIT);
        }
        self.redo.clear();
    }

    fn undo(&mut self, current: Snapshot) -> Option<Snapshot> {
        let prev = self.undo.pop()?;
        self.redo.push(current);
        Some(prev)
    }

    fn redo(&mut self, current: Snapshot) -> Option<Snapshot> {
        let next = self.redo.pop()?;
        self.undo.push(current);
        Some(next)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

/// Stroke widths offered by the toolbar (S / M / L).
const STROKE_SIZES: [f64; 3] = [1.5, 3.0, 6.0];

#[derive(Clone, Debug)]
pub struct TextInputState {
    pub client_x: f64,
    pub client_y: f64,
    pub ratio_x: f64,
    pub ratio_y: f64,
    /// Index of an existing `Shape::Text` being edited in place; `None`
    /// when the input will create a new text shape.
    pub editing: Option<usize>,
}

impl CanvasController {
    pub fn new() -> Self {
        Self {
            active_tool: RwSignal::new(DrawingTool::Pen),
            stroke_color: RwSignal::new("#ef4444".into()),
            stroke_width: RwSignal::new(3.0),
            shapes: RwSignal::new(Vec::new()),
            history: RwSignal::new(History::default()),
            gesture_base: StoredValue::new(None),
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

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            shapes: self.shapes.get_untracked(),
            selected: self.selected_idx.get_untracked(),
        }
    }

    /// The single path for undoable edits: records the current state, then
    /// installs `next`. A no-op edit records nothing.
    fn commit(&self, next: Vec<Shape>, selected: Option<usize>) {
        let before = self.snapshot();
        if next == before.shapes {
            return;
        }
        self.history.update(|h| h.record(before));
        self.shapes.set(next);
        self.selected_idx.set(selected);
        self.sync_pasted();
        self.poke_redraw();
    }

    /// Install a history entry (undo/redo/load) without recording one.
    fn apply(&self, snap: Snapshot) {
        // Any inline text edit refers to indices that are about to change.
        self.text_input.set(None);
        self.gesture_base.set_value(None);
        let selected = snap.selected.filter(|i| *i < snap.shapes.len());
        self.shapes.set(snap.shapes);
        self.sync_pasted();
        self.select(selected);
    }

    /// Image thumbnails are always derived from the image shapes, so undo,
    /// erase and delete can never leave a stale attachment behind.
    fn sync_pasted(&self) {
        let next = self
            .shapes
            .with_untracked(|s| Self::pasted_base64_for_shapes(s));
        if self.pasted_base64.with_untracked(|cur| *cur != next) {
            self.pasted_base64.set(next);
        }
    }

    /// Start a drag whose intermediate states should not hit history.
    fn begin_gesture(&self) {
        self.gesture_base.set_value(Some(self.snapshot()));
    }

    fn gesture_active(&self) -> bool {
        self.gesture_base.with_value(Option::is_some)
    }

    /// Finish a drag: record its starting state as one undo entry if the
    /// drag changed anything. Safe to call when no gesture is active.
    fn end_gesture(&self) {
        let Some(base) = self.gesture_base.with_value(|b| b.clone()) else {
            return;
        };
        self.gesture_base.set_value(None);
        let changed = self.shapes.with_untracked(|cur| *cur != base.shapes);
        if changed {
            self.history.update(|h| h.record(base));
            self.sync_pasted();
        }
    }

    pub fn add_shape(&self, shape: Shape) {
        let mut next = self.shapes.get_untracked();
        next.push(shape);
        self.commit(next, self.selected_idx.get_untracked());
    }

    pub fn undo(&self) {
        // Undo mid-drag: settle the drag first so it is what gets undone.
        self.end_gesture();
        let current = self.snapshot();
        let mut prev = None;
        self.history.update(|h| prev = h.undo(current));
        if let Some(prev) = prev {
            self.apply(prev);
        }
    }

    pub fn redo(&self) {
        self.end_gesture();
        let current = self.snapshot();
        let mut next = None;
        self.history.update(|h| next = h.redo(current));
        if let Some(next) = next {
            self.apply(next);
        }
    }

    /// User-initiated "Clear canvas" — undoable.
    pub fn clear_all(&self) {
        self.commit(Vec::new(), None);
    }

    /// Start completely fresh (after a submit, or discarding a draft): no
    /// shapes and no history, so Cmd+Z can't resurrect sent feedback.
    pub fn reset(&self) {
        self.load(Vec::new(), History::default());
    }

    /// Swap in another canvas's shapes + history (storyboard screen switch).
    pub fn load(&self, shapes: Vec<Shape>, history: History) {
        self.history.set(history);
        self.apply(Snapshot {
            shapes,
            selected: None,
        });
    }

    pub fn poke_redraw(&self) {
        self.redraw_trigger.update(|v| *v = v.wrapping_add(1));
    }

    pub fn redraw_token(&self) -> RwSignal<u64> {
        self.redraw_trigger
    }

    /// Render the canvas to a PNG data URL (without annotations already baked
    /// into the raster). Annotations are stored as vectors; we ask the canvas
    /// for its current pixel data, which *does* include drawn strokes because
    /// the canvas was the live render target.
    pub fn to_png_data_url(&self) -> Option<String> {
        self.canvas_node.with_value(|c| {
            let c = c.as_ref()?;
            // Selection box + handles are editing chrome, not annotation —
            // keep them out of the submitted bitmap, then restore them.
            render_shapes(c, self, None, false);
            let url = c.to_data_url_with_type("image/png").ok();
            render_all(c, self);
            url
        })
    }

    /// Small PNG preview of the current annotations (no selection chrome),
    /// at most `max_width` px wide, as raw base64 (no `data:` prefix).
    pub fn thumbnail_png_base64(&self, max_width: f64) -> Option<String> {
        self.canvas_node.with_value(|c| {
            let canvas = c.as_ref()?;
            let (w, h) = (canvas.width() as f64, canvas.height() as f64);
            if w < 1.0 || h < 1.0 {
                return None;
            }
            render_shapes(canvas, self, None, false);
            let scale = (max_width / w).min(1.0);
            let thumb = window()?
                .document()?
                .create_element("canvas")
                .ok()?
                .dyn_into::<HtmlCanvasElement>()
                .ok()?;
            thumb.set_width((w * scale).round().max(1.0) as u32);
            thumb.set_height((h * scale).round().max(1.0) as u32);
            let drawn = canvas_context(&thumb).and_then(|ctx| {
                // White backdrop: the live canvas is transparent by design,
                // but a preview card needs to be legible on any theme.
                ctx.set_fill_style_str("#ffffff");
                ctx.fill_rect(0.0, 0.0, w * scale, h * scale);
                ctx.draw_image_with_html_canvas_element_and_dw_and_dh(
                    canvas,
                    0.0,
                    0.0,
                    w * scale,
                    h * scale,
                )
                .ok()
            });
            render_all(canvas, self);
            drawn?;
            let url = thumb.to_data_url_with_type("image/png").ok()?;
            url.split_once(',').map(|(_, b64)| b64.to_string())
        })
    }

    pub fn annotations_json(&self) -> Vec<serde_json::Value> {
        Self::annotations_for_shapes(&self.shapes.get_untracked())
    }

    /// Serialize image geometry with a stable index into the accompanying
    /// pasted-image array. Legacy records without `assetIndex` remain
    /// loadable by image order.
    pub fn annotations_for_shapes(shapes: &[Shape]) -> Vec<serde_json::Value> {
        let mut image_index = 0_u64;
        shapes
            .iter()
            .map(|shape| {
                let mut value = shape.to_json();
                if matches!(shape, Shape::Image { .. }) {
                    if let Some(object) = value.as_object_mut() {
                        object.insert("assetIndex".into(), image_index.into());
                    }
                    image_index += 1;
                }
                value
            })
            .collect()
    }

    /// Derive attachment bytes from live image shapes so delete/crop edits
    /// cannot leave the old parallel `pasted_base64` array out of sync.
    pub fn pasted_base64_for_shapes(shapes: &[Shape]) -> Vec<String> {
        shapes
            .iter()
            .filter_map(|shape| match shape {
                Shape::Image { data_url, .. } => data_url.split_once(',').map(|(_, data)| data),
                _ => None,
            })
            .filter(|data| !data.is_empty())
            .map(str::to_string)
            .collect()
    }

    /// Hydrate persisted annotations and their attachment data URLs. Missing
    /// or corrupt image assets drop only that image shape; every vector shape
    /// remains editable.
    pub fn restore_annotations(
        annotations: &[serde_json::Value],
        image_data_urls: &[String],
    ) -> (Vec<Shape>, Vec<String>) {
        let mut legacy_image_index = 0_usize;
        let mut shapes = Vec::with_capacity(annotations.len());
        let mut restored_images = Vec::new();

        for value in annotations {
            let is_image = value.get("type").and_then(|v| v.as_str()) == Some("image");
            let image_url = if is_image {
                let index = value
                    .get("assetIndex")
                    .and_then(serde_json::Value::as_u64)
                    .map(|index| index as usize)
                    .unwrap_or(legacy_image_index);
                legacy_image_index += 1;
                image_data_urls.get(index).filter(|url| !url.is_empty())
            } else {
                None
            };

            if let Some(shape) = Shape::from_json(value, image_url.map(String::as_str)) {
                if let Shape::Image { data_url, .. } = &shape {
                    if let Some((_, data)) = data_url.split_once(',') {
                        restored_images.push(data.to_string());
                    }
                }
                shapes.push(shape);
            }
        }

        (shapes, restored_images)
    }

    pub fn cycle_stroke(&self, dir: i32) {
        let sizes = STROKE_SIZES;
        let current = self.stroke_width.get_untracked();
        let idx = sizes
            .iter()
            .position(|w| (*w - current).abs() < 0.01)
            .unwrap_or(1);
        let next = ((idx as i32 + dir).rem_euclid(sizes.len() as i32)) as usize;
        self.stroke_width.set(sizes[next]);
        self.restyle_selected(None, Some(sizes[next]));
    }

    /// Set the active color; when a shape is selected it is recolored too.
    pub fn set_stroke_color(&self, color: String) {
        self.stroke_color.set(color.clone());
        self.restyle_selected(Some(&color), None);
    }

    /// Select a shape (or clear the selection) and sync the toolbar's color /
    /// stroke pucks to the selected shape so they show what it looks like.
    pub fn select(&self, idx: Option<usize>) {
        self.selected_idx.set(idx);
        let style = idx.and_then(|i| {
            self.shapes
                .with_untracked(|shapes| shapes.get(i).and_then(shape_style))
        });
        if let Some((color, width)) = style {
            self.stroke_color.set(color);
            // Free-resized text can have an in-between size; only mirror
            // widths the S/M/L control can actually represent.
            if let Some(width) = width.filter(|w| STROKE_SIZES.iter().any(|p| (p - w).abs() < 0.01))
            {
                self.stroke_width.set(width);
            }
        }
        self.poke_redraw();
    }

    fn restyle_selected(&self, color: Option<&str>, width: Option<f64>) {
        let Some(idx) = self.selected_idx.get_untracked() else {
            return;
        };
        let Some(current) = self
            .shapes
            .with_untracked(|shapes| shapes.get(idx).cloned())
        else {
            return;
        };
        let next = restyle_shape(&current, color, width);
        if next != current {
            self.replace_shape(idx, next, true);
        }
    }

    pub fn delete_selected(&self) {
        let Some(idx) = self.selected_idx.get_untracked() else {
            return;
        };
        let mut next = self.shapes.get_untracked();
        if idx >= next.len() {
            self.selected_idx.set(None);
            return;
        }
        next.remove(idx);
        self.commit(next, None);
    }

    /// Replace one shape. `record = true` is an undoable edit; `false` is a
    /// live update inside a gesture (drag/resize), which only takes effect
    /// while that gesture is active — a stale mousemove after undo is a no-op.
    pub fn replace_shape(&self, idx: usize, shape: Shape, record: bool) {
        let mut next = self.shapes.get_untracked();
        if idx >= next.len() {
            return;
        }
        next[idx] = shape;
        if record {
            self.commit(next, self.selected_idx.get_untracked());
        } else if self.gesture_active() {
            self.shapes.set(next);
            self.poke_redraw();
        }
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
    /// For Select tool: dragging a resize handle of shape `idx`; the shape
    /// prior to the resize is kept so the gesture is one undo step.
    resize: Option<(usize, Handle, Shape)>,
}

#[component]
pub fn CanvasSurface(controller: CanvasController) -> impl IntoView {
    let canvas_node: NodeRef<Canvas> = NodeRef::new();
    let pointer: StoredValue<PointerState> = StoredValue::new(PointerState::default());
    let text_input_value: RwSignal<String> = RwSignal::new(String::new());
    // Select-tool hover feedback (resize / move cursors); falls back to the
    // active tool's cursor when nothing editable is under the pointer.
    let hover_cursor: RwSignal<Option<&'static str>> = RwSignal::new(None);
    let text_input_node: NodeRef<Input> = NodeRef::new();

    // `autofocus` is ignored for inputs inserted after page load, so focus
    // the text box explicitly whenever it mounts. When editing existing text,
    // preselect it so typing replaces the old words.
    {
        let ctrl = controller.clone();
        Effect::new(move |_| {
            if let Some(input) = text_input_node.get() {
                let _ = input.focus();
                let editing = ctrl
                    .text_input
                    .with_untracked(|s| s.as_ref().is_some_and(|s| s.editing.is_some()));
                if editing {
                    input.select();
                }
            }
        });
    }

    // Open the inline text input on top of an existing text shape so it can
    // be rewritten in place.
    let begin_text_edit = {
        let ctrl = controller.clone();
        move |idx: usize, rect: &DomRect| {
            let Some(Shape::Text { x, y, text, .. }) = ctrl
                .shapes
                .with_untracked(|shapes| shapes.get(idx).cloned())
            else {
                return;
            };
            ctrl.select(Some(idx));
            ctrl.text_input.set(Some(TextInputState {
                client_x: x * rect.width(),
                client_y: y * rect.height(),
                ratio_x: x,
                ratio_y: y,
                editing: Some(idx),
            }));
            text_input_value.set(text);
        }
    };

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
                let _ = win.add_event_listener_with_callback("resize", cb.as_ref().unchecked_ref());
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
        let begin_text_edit = begin_text_edit.clone();
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
                // Clicking existing text with the Text tool edits it.
                let shapes = ctrl.shapes.get_untracked();
                if let Some(idx) = pick_shape_at(&shapes, rx, ry)
                    .filter(|i| matches!(shapes.get(*i), Some(Shape::Text { .. })))
                {
                    begin_text_edit(idx, &rect);
                    return;
                }
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
                    editing: None,
                }));
                text_input_value.set(String::new());
                return;
            }

            if tool == DrawingTool::Select {
                let shapes = ctrl.shapes.get_untracked();
                // A handle of the already-selected shape wins over picking,
                // so corners that overlap other shapes stay grabbable.
                let resize = ctrl.selected_idx.get_untracked().and_then(|i| {
                    let shape = shapes.get(i)?;
                    handle_at(shape, rx, ry, handle_tolerance(&rect))
                        .map(|handle| (i, handle, shape.clone()))
                });
                let picked = match &resize {
                    Some((i, _, _)) => Some(*i),
                    None => pick_shape_at(&shapes, rx, ry),
                };
                if resize.is_none() {
                    ctrl.select(picked);
                }
                if picked.is_some() {
                    ctrl.begin_gesture();
                }
                pointer.update_value(|state| {
                    state.is_down = true;
                    state.start = (rx, ry);
                    state.current = vec![(rx, ry)];
                    state.preview = None;
                    state.drag_original = if resize.is_some() {
                        None
                    } else {
                        picked.and_then(|i| shapes.get(i).cloned().map(|s| (i, s)))
                    };
                    state.crop_target = None;
                    state.resize = resize;
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
                    state.resize = None;
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
                state.resize = None;
            });

            if tool == DrawingTool::Eraser {
                ctrl.begin_gesture();
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
            if !is_down {
                let cursor = if tool == DrawingTool::Select {
                    select_hover_cursor(&ctrl, rx, ry, handle_tolerance(&rect))
                } else {
                    None
                };
                if hover_cursor.get_untracked() != cursor {
                    hover_cursor.set(cursor);
                }
                return;
            }
            if tool == DrawingTool::Text {
                return;
            }

            if tool == DrawingTool::Eraser {
                erase_at(&ctrl, rx, ry, width);
                return;
            }

            if tool == DrawingTool::Select {
                if let Some((idx, handle, original)) = pointer.with_value(|s| s.resize.clone()) {
                    // Live update without pushing undo; undo is recorded on mouseup.
                    ctrl.replace_shape(idx, resize_shape(&original, handle, (rx, ry)), false);
                    return;
                }
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
            // Close any move/resize/erase gesture regardless of the current
            // tool — a shortcut pressed mid-drag must not leave it open.
            ctrl.end_gesture();

            if tool == DrawingTool::Select {
                pointer.update_value(|state| {
                    state.is_down = false;
                    state.drag_original = None;
                    state.preview = None;
                    state.resize = None;
                });
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
                if let (
                    Some(idx),
                    Some(Shape::Rect {
                        x, y, w: rw, h: rh, ..
                    }),
                ) = (target, preview)
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
            let Some(state) = ctrl.text_input.get_untracked() else {
                return;
            };
            let text = value.get_untracked().trim().to_string();
            ctrl.text_input.set(None);
            if let Some(idx) = state.editing {
                let current = ctrl
                    .shapes
                    .with_untracked(|shapes| shapes.get(idx).cloned());
                let Some(Shape::Text {
                    x,
                    y,
                    color,
                    font_size,
                    text: old,
                }) = current
                else {
                    return;
                };
                if text.is_empty() {
                    // Clearing the text removes the shape (undoable).
                    ctrl.selected_idx.set(Some(idx));
                    ctrl.delete_selected();
                } else if text != old {
                    ctrl.replace_shape(
                        idx,
                        Shape::Text {
                            x,
                            y,
                            text,
                            color,
                            font_size,
                        },
                        true,
                    );
                }
                return;
            }
            if text.is_empty() {
                return;
            }
            let width = ctrl.stroke_width.get_untracked();
            ctrl.add_shape(Shape::Text {
                x: state.ratio_x,
                y: state.ratio_y,
                text,
                color: ctrl.stroke_color.get_untracked(),
                font_size: text_font_size(width),
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
                        })
                            as Box<dyn FnMut(Event)>);
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

    // Double-click a text shape with the Select tool to edit its words.
    let on_dblclick = {
        let ctrl = controller.clone();
        move |ev: MouseEvent| {
            if ctrl.active_tool.get_untracked() != DrawingTool::Select {
                return;
            }
            let Some(el) = canvas_node.get() else { return };
            let canvas: HtmlCanvasElement = el.into();
            let rect = canvas.get_bounding_client_rect();
            let (rx, ry) = event_ratio(&ev, &rect);
            let shapes = ctrl.shapes.get_untracked();
            if let Some(idx) = pick_shape_at(&shapes, rx, ry)
                .filter(|i| matches!(shapes.get(*i), Some(Shape::Text { .. })))
            {
                ev.prevent_default();
                begin_text_edit(idx, &rect);
            }
        }
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
                style:cursor=move || {
                    hover_cursor
                        .get()
                        .unwrap_or_else(|| controller.active_tool.get().cursor())
                }
                on:mousedown=on_mousedown
                on:dblclick=on_dblclick
                on:mousemove=on_mousemove
                on:mouseup=on_mouseup.clone()
                // Releasing outside the canvas never delivers a mouseup here;
                // finish the stroke/drag on leave so nothing stays stuck.
                on:mouseleave=on_mouseup
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
                            node_ref=text_input_node
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
                        <div>"Press V to select a shape, then move, resize, or recolor it."</div>
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
    CANVAS_PX.with(|c| c.set((w as f64, h as f64)));
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
    let cb = Closure::wrap(
        Box::new(move |_entries: js_sys::Array, _obs: ResizeObserver| {
            fit_canvas_to_viewport(&canvas_for_cb);
            render_all(&canvas_for_cb, &ctrl);
        }) as Box<dyn FnMut(js_sys::Array, ResizeObserver)>,
    );
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
    render_shapes(canvas, ctrl, preview, true);
}

fn render_shapes(
    canvas: &HtmlCanvasElement,
    ctrl: &CanvasController,
    preview: Option<&Shape>,
    show_selection: bool,
) {
    // Keep bitmap in sync with displayed CSS box. This is a no-op whenever the
    // size hasn't drifted, but protects against cases where the ResizeObserver
    // hasn't caught up yet (first frame after mount, window resize mid-drag).
    fit_canvas_to_viewport(canvas);
    let Some(ctx) = canvas_context(canvas) else {
        return;
    };
    let w = canvas.width() as f64;
    let h = canvas.height() as f64;

    ctx.clear_rect(0.0, 0.0, w, h);

    let shapes = ctrl.shapes.get_untracked();
    let selected = ctrl.selected_idx.get_untracked();
    for shape in shapes.iter() {
        render_shape(&ctx, shape, w, h);
    }
    // Selection chrome goes on top of every shape so handles stay grabbable.
    if let Some(shape) = selected
        .filter(|_| show_selection)
        .and_then(|i| shapes.get(i))
    {
        if let Some((bx, by, bw, bh)) = shape_bounds(shape) {
            draw_selection_box(&ctx, bx * w, by * h, bw * w, bh * h);
        }
        for (handle, (hx, hy)) in shape_handles(shape) {
            draw_handle(&ctx, handle, hx * w, hy * h);
        }
    }
    if let Some(shape) = preview {
        render_shape(&ctx, shape, w, h);
    }
}

fn draw_handle(ctx: &CanvasRenderingContext2d, handle: Handle, x: f64, y: f64) {
    ctx.save();
    ctx.set_fill_style_str("#ffffff");
    ctx.set_stroke_style_str("#2563eb");
    ctx.set_line_width(1.5);
    ctx.begin_path();
    match handle {
        Handle::Start | Handle::End => {
            let _ = ctx.arc(x, y, HANDLE_PX / 2.0 + 1.0, 0.0, std::f64::consts::TAU);
        }
        _ => ctx.rect(
            x - HANDLE_PX / 2.0,
            y - HANDLE_PX / 2.0,
            HANDLE_PX,
            HANDLE_PX,
        ),
    }
    ctx.fill();
    ctx.stroke();
    ctx.restore();
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
        Shape::Path {
            points,
            color,
            width,
        } => {
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
        Shape::Line {
            from,
            to,
            color,
            width,
        } => {
            ctx.set_stroke_style_str(color);
            ctx.set_line_width(*width);
            ctx.set_line_cap("round");
            ctx.begin_path();
            ctx.move_to(from.0 * w, from.1 * h);
            ctx.line_to(to.0 * w, to.1 * h);
            ctx.stroke();
        }
        Shape::Ellipse {
            x,
            y,
            w: rw,
            h: rh,
            color,
            width,
        } => {
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
        Shape::Arrow {
            from,
            to,
            color,
            width,
        } => {
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
        Shape::Rect {
            x,
            y,
            w: rw,
            h: rh,
            color,
            width,
        } => {
            ctx.set_stroke_style_str(color);
            ctx.set_line_width(*width);
            ctx.stroke_rect(x * w, y * h, rw * w, rh * h);
        }
        Shape::Highlight {
            x,
            y,
            w: rw,
            h: rh,
            color,
        } => {
            ctx.set_global_alpha(0.35);
            ctx.set_fill_style_str(color);
            ctx.fill_rect(x * w, y * h, rw * w, rh * h);
        }
        Shape::Text {
            x,
            y,
            text,
            color,
            font_size,
        } => {
            ctx.set_fill_style_str(color);
            ctx.set_font(&text_font(*font_size));
            ctx.set_text_baseline("top");
            let _ = ctx.fill_text(text, x * w, y * h);
        }
        Shape::Image {
            x,
            y,
            w: iw,
            h: ih,
            data_url,
        } => {
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
    // Erasing happens live inside an eraser gesture; the whole stroke is
    // recorded as one undo entry on mouseup.
    if !ctrl.gesture_active() {
        return;
    }
    let radius_r = 0.02 * (width.max(1.0) / 2.0);
    let shapes = ctrl.shapes.get_untracked();
    let before = shapes.len();
    let filtered: Vec<Shape> = shapes
        .into_iter()
        .filter(|s| !shape_hit(s, rx, ry, radius_r))
        .collect();
    if filtered.len() != before {
        // Indices shifted; a stale selection would point at the wrong shape.
        ctrl.selected_idx.set(None);
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
        Shape::Text { .. } => shape_bounds(shape).is_some_and(|(x, y, w, h)| {
            rx >= x - r && rx <= x + w + r && ry >= y - r && ry <= y + h + r
        }),
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
        Shape::Text {
            x,
            y,
            text,
            font_size,
            ..
        } => {
            let (w, h) = text_extent(text, *font_size);
            Some((*x, *y, w, h))
        }
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

/// Handle hit radius in ratio coords for the canvas's on-screen size.
fn handle_tolerance(rect: &DomRect) -> (f64, f64) {
    (
        HANDLE_PX / rect.width().max(1.0),
        HANDLE_PX / rect.height().max(1.0),
    )
}

/// Cursor for the Select tool hovering at (rx, ry): a resize cursor over a
/// handle of the selected shape, `move` over any shape, otherwise `None`.
fn select_hover_cursor(
    ctrl: &CanvasController,
    rx: f64,
    ry: f64,
    tol: (f64, f64),
) -> Option<&'static str> {
    ctrl.shapes.with_untracked(|shapes| {
        let on_handle = ctrl
            .selected_idx
            .get_untracked()
            .and_then(|i| shapes.get(i))
            .and_then(|shape| handle_at(shape, rx, ry, tol));
        match on_handle {
            Some(handle) => Some(handle.cursor()),
            None => pick_shape_at(shapes, rx, ry).map(|_| "move"),
        }
    })
}

fn translate_shape(shape: &Shape, dx: f64, dy: f64) -> Shape {
    let t = |(a, b): (f64, f64)| (a + dx, b + dy);
    match shape {
        Shape::Path {
            points,
            color,
            width,
        } => Shape::Path {
            points: points.iter().map(|p| t(*p)).collect(),
            color: color.clone(),
            width: *width,
        },
        Shape::Line {
            from,
            to,
            color,
            width,
        } => Shape::Line {
            from: t(*from),
            to: t(*to),
            color: color.clone(),
            width: *width,
        },
        Shape::Arrow {
            from,
            to,
            color,
            width,
        } => Shape::Arrow {
            from: t(*from),
            to: t(*to),
            color: color.clone(),
            width: *width,
        },
        Shape::Rect {
            x,
            y,
            w,
            h,
            color,
            width,
        } => Shape::Rect {
            x: x + dx,
            y: y + dy,
            w: *w,
            h: *h,
            color: color.clone(),
            width: *width,
        },
        Shape::Ellipse {
            x,
            y,
            w,
            h,
            color,
            width,
        } => Shape::Ellipse {
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
        Shape::Text {
            x,
            y,
            text,
            color,
            font_size,
        } => Shape::Text {
            x: x + dx,
            y: y + dy,
            text: text.clone(),
            color: color.clone(),
            font_size: *font_size,
        },
        Shape::Image {
            x,
            y,
            w,
            h,
            data_url,
        } => Shape::Image {
            x: x + dx,
            y: y + dy,
            w: *w,
            h: *h,
            data_url: data_url.clone(),
        },
    }
}

// ---------------------------------------------------------------------------
// Shape editing — resize handles + restyle (pure geometry, unit-tested)
// ---------------------------------------------------------------------------

thread_local! {
    /// Last known canvas size in px. Shapes live in 0..1 ratio space, but
    /// text has a pixel font size, so its ratio bounds depend on this.
    static CANVAS_PX: std::cell::Cell<(f64, f64)> = const { std::cell::Cell::new((0.0, 0.0)) };
}

/// Line height multiplier for single-line canvas text (baseline = top).
const TEXT_LINE_HEIGHT: f64 = 1.2;

fn text_font(font_size: f64) -> String {
    format!("{font_size}px -apple-system, sans-serif")
}

/// Ratio-space (width, height) a text shape occupies on the current canvas,
/// measured with the same font the renderer uses.
fn text_extent(text: &str, font_size: f64) -> (f64, f64) {
    let (cw, ch) = CANVAS_PX.with(|c| c.get());
    // Before first layout (and in native unit tests) assume a typical window.
    let (cw, ch) = if cw > 0.0 && ch > 0.0 {
        (cw, ch)
    } else {
        (1280.0, 800.0)
    };
    let width_px = measure_text_px(text, font_size)
        .unwrap_or_else(|| text.chars().count() as f64 * font_size * 0.55);
    (
        width_px.max(font_size * 0.5) / cw,
        font_size * TEXT_LINE_HEIGHT / ch,
    )
}

#[cfg(target_arch = "wasm32")]
fn measure_text_px(text: &str, font_size: f64) -> Option<f64> {
    thread_local! {
        static MEASURE_CTX: Option<CanvasRenderingContext2d> = window()
            .and_then(|w| w.document())
            .and_then(|d| d.create_element("canvas").ok())
            .and_then(|el| el.dyn_into::<HtmlCanvasElement>().ok())
            .and_then(|c| canvas_context(&c));
    }
    MEASURE_CTX.with(|ctx| {
        let ctx = ctx.as_ref()?;
        ctx.set_font(&text_font(font_size));
        ctx.measure_text(text).ok().map(|m| m.width())
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn measure_text_px(_text: &str, _font_size: f64) -> Option<f64> {
    None
}

/// On-screen size of a resize handle, in CSS px.
const HANDLE_PX: f64 = 9.0;
/// Font-size range a text resize is clamped to, in px.
const MIN_FONT_PX: f64 = 8.0;
const MAX_FONT_PX: f64 = 160.0;
/// Smallest width/height (ratio coords) a resize can shrink a shape to.
const MIN_SHAPE_SIZE: f64 = 0.004;

/// A grab point on the selected shape. Box-like shapes (including text)
/// get four corners; lines/arrows get their two endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Handle {
    Nw,
    Ne,
    Se,
    Sw,
    Start,
    End,
}

impl Handle {
    fn cursor(self) -> &'static str {
        match self {
            Self::Nw | Self::Se => "nwse-resize",
            Self::Ne | Self::Sw => "nesw-resize",
            Self::Start | Self::End => "grab",
        }
    }
}

fn shape_handles(shape: &Shape) -> Vec<(Handle, (f64, f64))> {
    match shape {
        Shape::Line { from, to, .. } | Shape::Arrow { from, to, .. } => {
            vec![(Handle::Start, *from), (Handle::End, *to)]
        }
        _ => match shape_bounds(shape) {
            Some((x, y, w, h)) => vec![
                (Handle::Nw, (x, y)),
                (Handle::Ne, (x + w, y)),
                (Handle::Se, (x + w, y + h)),
                (Handle::Sw, (x, y + h)),
            ],
            None => Vec::new(),
        },
    }
}

/// Which handle of `shape` (if any) lies under the pointer. `tol` is the
/// hit radius in ratio coords per axis.
fn handle_at(shape: &Shape, rx: f64, ry: f64, tol: (f64, f64)) -> Option<Handle> {
    shape_handles(shape)
        .into_iter()
        .find(|(_, (hx, hy))| (rx - hx).abs() <= tol.0 && (ry - hy).abs() <= tol.1)
        .map(|(handle, _)| handle)
}

/// Resize `original` by dragging `handle` to `p`. Corners keep the opposite
/// corner anchored; images and text scale uniformly (text by font size);
/// endpoints move freely.
fn resize_shape(original: &Shape, handle: Handle, p: (f64, f64)) -> Shape {
    match (original, handle) {
        (
            Shape::Line {
                from,
                to,
                color,
                width,
            },
            Handle::Start | Handle::End,
        ) => {
            let (from, to) = if handle == Handle::Start {
                (p, *to)
            } else {
                (*from, p)
            };
            return Shape::Line {
                from,
                to,
                color: color.clone(),
                width: *width,
            };
        }
        (
            Shape::Arrow {
                from,
                to,
                color,
                width,
            },
            Handle::Start | Handle::End,
        ) => {
            let (from, to) = if handle == Handle::Start {
                (p, *to)
            } else {
                (*from, p)
            };
            return Shape::Arrow {
                from,
                to,
                color: color.clone(),
                width: *width,
            };
        }
        _ => {}
    }
    let Some(old @ (x, y, w, h)) = shape_bounds(original) else {
        return original.clone();
    };
    let anchor = match handle {
        Handle::Nw => (x + w, y + h),
        Handle::Ne => (x, y + h),
        Handle::Se => (x, y),
        Handle::Sw => (x + w, y),
        Handle::Start | Handle::End => return original.clone(),
    };
    let mut nw = (p.0 - anchor.0).abs().max(MIN_SHAPE_SIZE);
    let mut nh = (p.1 - anchor.1).abs().max(MIN_SHAPE_SIZE);
    if matches!(original, Shape::Image { .. } | Shape::Text { .. }) && w > 0.0 && h > 0.0 {
        let scale = (nw / w).max(nh / h);
        nw = w * scale;
        nh = h * scale;
    }
    let nx = if p.0 < anchor.0 {
        anchor.0 - nw
    } else {
        anchor.0
    };
    let ny = if p.1 < anchor.1 {
        anchor.1 - nh
    } else {
        anchor.1
    };
    fit_shape_to_bounds(original, old, (nx, ny, nw, nh))
}

/// Map a shape from its `old` bounding box onto `new`. Freehand paths are
/// scaled point-by-point; a degenerate axis (e.g. a perfectly vertical
/// stroke) is translated instead of scaled.
fn fit_shape_to_bounds(
    shape: &Shape,
    old: (f64, f64, f64, f64),
    new: (f64, f64, f64, f64),
) -> Shape {
    let (nx, ny, nw, nh) = new;
    match shape {
        Shape::Rect { color, width, .. } => Shape::Rect {
            x: nx,
            y: ny,
            w: nw,
            h: nh,
            color: color.clone(),
            width: *width,
        },
        Shape::Ellipse { color, width, .. } => Shape::Ellipse {
            x: nx,
            y: ny,
            w: nw,
            h: nh,
            color: color.clone(),
            width: *width,
        },
        Shape::Highlight { color, .. } => Shape::Highlight {
            x: nx,
            y: ny,
            w: nw,
            h: nh,
            color: color.clone(),
        },
        Shape::Image { data_url, .. } => Shape::Image {
            x: nx,
            y: ny,
            w: nw,
            h: nh,
            data_url: data_url.clone(),
        },
        Shape::Path {
            points,
            color,
            width,
        } => {
            let (ox, oy, ow, oh) = old;
            let map = |v: f64, o: f64, os: f64, n: f64, ns: f64| {
                if os > 1e-6 {
                    n + (v - o) / os * ns
                } else {
                    v - o + n
                }
            };
            Shape::Path {
                points: points
                    .iter()
                    .map(|(px, py)| (map(*px, ox, ow, nx, nw), map(*py, oy, oh, ny, nh)))
                    .collect(),
                color: color.clone(),
                width: *width,
            }
        }
        Shape::Text {
            text,
            color,
            font_size,
            ..
        } => {
            let scale = if old.3 > 1e-9 { nh / old.3 } else { 1.0 };
            Shape::Text {
                x: nx,
                y: ny,
                text: text.clone(),
                color: color.clone(),
                font_size: (font_size * scale).clamp(MIN_FONT_PX, MAX_FONT_PX),
            }
        }
        Shape::Line { .. } | Shape::Arrow { .. } => shape.clone(),
    }
}

/// Font size used for text drawn at a given stroke width.
fn text_font_size(stroke_width: f64) -> f64 {
    14.0 + stroke_width * 2.0
}

/// Apply a new color and/or stroke width to a shape. Images have no style;
/// highlights have no stroke; text maps stroke width to font size.
fn restyle_shape(shape: &Shape, new_color: Option<&str>, new_width: Option<f64>) -> Shape {
    let mut next = shape.clone();
    match &mut next {
        Shape::Path { color, width, .. }
        | Shape::Line { color, width, .. }
        | Shape::Arrow { color, width, .. }
        | Shape::Rect { color, width, .. }
        | Shape::Ellipse { color, width, .. } => {
            if let Some(c) = new_color {
                *color = c.to_string();
            }
            if let Some(w) = new_width {
                *width = w;
            }
        }
        Shape::Highlight { color, .. } => {
            if let Some(c) = new_color {
                *color = c.to_string();
            }
        }
        Shape::Text {
            color, font_size, ..
        } => {
            if let Some(c) = new_color {
                *color = c.to_string();
            }
            if let Some(w) = new_width {
                *font_size = text_font_size(w);
            }
        }
        Shape::Image { .. } => {}
    }
    next
}

/// The (color, stroke width) a shape was drawn with, so the toolbar can
/// reflect the current selection. `None` for images.
fn shape_style(shape: &Shape) -> Option<(String, Option<f64>)> {
    match shape {
        Shape::Path { color, width, .. }
        | Shape::Line { color, width, .. }
        | Shape::Arrow { color, width, .. }
        | Shape::Rect { color, width, .. }
        | Shape::Ellipse { color, width, .. } => Some((color.clone(), Some(*width))),
        Shape::Highlight { color, .. } => Some((color.clone(), None)),
        Shape::Text {
            color, font_size, ..
        } => Some((color.clone(), Some((font_size - 14.0) / 2.0))),
        Shape::Image { .. } => None,
    }
}

/// Crop the Image at `idx` to the canvas-ratio rect (cx, cy, cw, ch),
/// clamped to the image's own bounds. Produces a new data URL via an
/// offscreen canvas and replaces the shape in place, recording an undo entry.
async fn perform_crop(ctrl: &CanvasController, idx: usize, cx: f64, cy: f64, cw: f64, ch: f64) {
    let Some(original) = ctrl.shapes.with_untracked(|s| s.get(idx).cloned()) else {
        return;
    };
    let Shape::Image {
        x: ix,
        y: iy,
        w: iw,
        h: ih,
        data_url,
    } = original.clone()
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

    let Some(doc) = window().and_then(|w| w.document()) else {
        return;
    };
    let off: HtmlCanvasElement = match doc.create_element("canvas").and_then(|e| {
        e.dyn_into::<HtmlCanvasElement>()
            .map_err(|_| wasm_bindgen::JsValue::NULL.into())
    }) {
        Ok(c) => c,
        Err(_) => return,
    };
    off.set_width(sw.max(1.0) as u32);
    off.set_height(sh.max(1.0) as u32);
    let Some(ctx) = canvas_context(&off) else {
        return;
    };
    if ctx
        .draw_image_with_html_image_element_and_sw_and_sh_and_dx_and_dy_and_dw_and_dh(
            &img, sx, sy, sw, sh, 0.0, 0.0, sw, sh,
        )
        .is_err()
    {
        return;
    }
    let Ok(new_url) = off.to_data_url_with_type("image/png") else {
        return;
    };

    let new_shape = Shape::Image {
        x: rx0,
        y: ry0,
        w: rx1 - rx0,
        h: ry1 - ry0,
        data_url: new_url.into(),
    };
    // The decode above is async: if the user undid, deleted or moved the
    // image meanwhile, `idx` may now name a different shape — bail rather
    // than overwrite it.
    if ctrl
        .shapes
        .with_untracked(|s| s.get(idx) != Some(&original))
    {
        return;
    }
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
    // `pasted_base64` (thumbnails) is derived from image shapes when the
    // shape below is committed, so it can never drift from the canvas.
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
                    x: 0.3,
                    y: 0.3,
                    w: 0.4,
                    h: 0.4,
                    data_url: data_for_resolve.into(),
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
                x,
                y,
                w: iw,
                h: ih,
                data_url: data_for_resolve.into(),
            });
        } else {
            ctrl2.add_shape(Shape::Image {
                x: 0.3,
                y: 0.3,
                w: 0.4,
                h: 0.4,
                data_url: data_for_resolve.into(),
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
            let Some(ev) = ev.dyn_ref::<ClipboardEvent>() else {
                return;
            };
            let Some(data) = ev.clipboard_data() else {
                return;
            };
            let Some(items) = data
                .items()
                .dyn_ref::<web_sys::DataTransferItemList>()
                .cloned()
            else {
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
                        })
                            as Box<dyn FnMut(Event)>);
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
            let Some(kev) = ev.dyn_ref::<KeyboardEvent>() else {
                return;
            };
            // Leave every shortcut — including Cmd+Z — to the field when the
            // user is typing in an input / textarea / contenteditable, so
            // undoing a typo in the description never undoes the drawing.
            if is_editable_target(kev) {
                return;
            }
            let cmd_or_ctrl = kev.meta_key() || kev.ctrl_key();
            let key_lower = kev.key().to_lowercase();
            if cmd_or_ctrl && !kev.alt_key() {
                let redo = (key_lower == "z" && kev.shift_key()) || key_lower == "y";
                if redo {
                    ctrl.redo();
                    kev.prevent_default();
                    return;
                }
                if key_lower == "z" {
                    ctrl.undo();
                    kev.prevent_default();
                    return;
                }
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
                    ctrl.select(None);
                }
                _ => {}
            }
        }) as Box<dyn FnMut(Event)>);
        let _ = win.add_event_listener_with_callback("keydown", cb.as_ref().unchecked_ref());
        cb.forget();
    });
}

fn is_editable_target(ev: &KeyboardEvent) -> bool {
    let Some(target) = ev.target() else {
        return false;
    };
    let Some(el) = target.dyn_ref::<web_sys::Element>() else {
        return false;
    };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annotation_round_trip_restores_vectors_and_indexed_images() {
        let shapes = vec![
            Shape::Rect {
                x: 0.1,
                y: 0.2,
                w: 0.3,
                h: 0.4,
                color: "#123456".into(),
                width: 6.0,
            },
            Shape::Image {
                x: 0.2,
                y: 0.3,
                w: 0.4,
                h: 0.5,
                data_url: "data:image/png;base64,AAAA".into(),
            },
        ];
        let annotations = CanvasController::annotations_for_shapes(&shapes);
        assert_eq!(annotations[1]["assetIndex"], 0);

        let urls = vec!["data:image/png;base64,AAAA".to_string()];
        let (restored, pasted) = CanvasController::restore_annotations(&annotations, &urls);
        assert_eq!(restored.len(), 2);
        assert_eq!(pasted, vec!["AAAA"]);
        assert!(matches!(
            restored[0],
            Shape::Rect { x, y, w, h, .. }
                if x == 0.1 && y == 0.2 && w == 0.3 && h == 0.4
        ));
        assert!(matches!(
            &restored[1],
            Shape::Image { data_url, .. } if &**data_url == "data:image/png;base64,AAAA"
        ));
    }

    #[test]
    fn legacy_images_restore_by_shape_order() {
        let annotations = vec![
            json!({"type":"image", "x":0.0, "y":0.0, "w":0.5, "h":0.5}),
            json!({"type":"image", "x":0.5, "y":0.5, "w":0.5, "h":0.5}),
        ];
        let urls = vec![
            "data:image/png;base64,ONE".to_string(),
            "data:image/png;base64,TWO".to_string(),
        ];
        let (restored, pasted) = CanvasController::restore_annotations(&annotations, &urls);
        assert_eq!(restored.len(), 2);
        assert_eq!(pasted, vec!["ONE", "TWO"]);
    }

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Shape {
        Shape::Rect {
            x,
            y,
            w,
            h,
            color: "#ef4444".into(),
            width: 3.0,
        }
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn corner_resize_keeps_opposite_corner_anchored() {
        let resized = resize_shape(&rect(0.2, 0.2, 0.2, 0.2), Handle::Se, (0.6, 0.5));
        let Shape::Rect { x, y, w, h, .. } = resized else {
            panic!("not a rect")
        };
        assert!(approx(x, 0.2) && approx(y, 0.2) && approx(w, 0.4) && approx(h, 0.3));

        let resized = resize_shape(&rect(0.2, 0.2, 0.2, 0.2), Handle::Nw, (0.1, 0.3));
        let Shape::Rect { x, y, w, h, .. } = resized else {
            panic!("not a rect")
        };
        assert!(approx(x, 0.1) && approx(y, 0.3) && approx(w, 0.3) && approx(h, 0.1));
    }

    #[test]
    fn resize_past_anchor_flips_and_never_collapses() {
        let resized = resize_shape(&rect(0.2, 0.2, 0.2, 0.2), Handle::Se, (0.2, 0.1));
        let Shape::Rect { x, y, w, h, .. } = resized else {
            panic!("not a rect")
        };
        assert!(approx(x, 0.2) && approx(w, MIN_SHAPE_SIZE));
        assert!(approx(y, 0.1) && approx(h, 0.1));
    }

    #[test]
    fn image_resize_preserves_aspect_ratio() {
        let img = Shape::Image {
            x: 0.0,
            y: 0.0,
            w: 0.4,
            h: 0.2,
            data_url: "data:,".into(),
        };
        let Shape::Image { w, h, .. } = resize_shape(&img, Handle::Se, (0.8, 0.25)) else {
            panic!("not an image")
        };
        assert!(approx(w / h, 2.0));
        assert!(approx(w, 0.8));
    }

    #[test]
    fn path_scales_into_new_bounds() {
        let path = Shape::Path {
            points: vec![(0.1, 0.1), (0.3, 0.2)],
            color: "#000".into(),
            width: 1.5,
        };
        let Shape::Path { points, .. } = resize_shape(&path, Handle::Se, (0.5, 0.3)) else {
            panic!("not a path")
        };
        assert!(approx(points[0].0, 0.1) && approx(points[0].1, 0.1));
        assert!(approx(points[1].0, 0.5) && approx(points[1].1, 0.3));
    }

    #[test]
    fn line_endpoints_move_independently() {
        let arrow = Shape::Arrow {
            from: (0.1, 0.1),
            to: (0.5, 0.5),
            color: "#000".into(),
            width: 3.0,
        };
        assert_eq!(shape_handles(&arrow).len(), 2);
        let Shape::Arrow { from, to, .. } = resize_shape(&arrow, Handle::End, (0.9, 0.2)) else {
            panic!("not an arrow")
        };
        assert_eq!(from, (0.1, 0.1));
        assert_eq!(to, (0.9, 0.2));
        assert_eq!(
            handle_at(&arrow, 0.502, 0.499, (0.01, 0.01)),
            Some(Handle::End)
        );
        assert_eq!(handle_at(&arrow, 0.3, 0.3, (0.01, 0.01)), None);
    }

    #[test]
    fn restyle_changes_color_and_width_but_not_images() {
        let restyled = restyle_shape(&rect(0.0, 0.0, 0.1, 0.1), Some("#2563eb"), Some(6.0));
        assert_eq!(
            shape_style(&restyled),
            Some(("#2563eb".to_string(), Some(6.0)))
        );

        let text = Shape::Text {
            x: 0.0,
            y: 0.0,
            text: "hi".into(),
            color: "#000".into(),
            font_size: 20.0,
        };
        let restyled = restyle_shape(&text, None, Some(6.0));
        assert_eq!(
            shape_style(&restyled),
            Some(("#000".to_string(), Some(6.0)))
        );

        let img = Shape::Image {
            x: 0.0,
            y: 0.0,
            w: 0.1,
            h: 0.1,
            data_url: "data:,".into(),
        };
        assert_eq!(restyle_shape(&img, Some("#fff"), Some(1.5)), img);
    }

    fn snap(n: usize) -> Snapshot {
        Snapshot {
            shapes: (0..n)
                .map(|i| rect(i as f64 * 0.1, 0.0, 0.05, 0.05))
                .collect(),
            selected: n.checked_sub(1),
        }
    }

    #[test]
    fn history_undo_redo_round_trip() {
        let mut h = History::default();
        h.record(snap(0));
        h.record(snap(1));
        // current state is snap(2)
        assert_eq!(h.undo(snap(2)), Some(snap(1)));
        assert_eq!(h.undo(snap(1)), Some(snap(0)));
        assert_eq!(h.undo(snap(0)), None);
        assert!(h.can_redo());
        assert_eq!(h.redo(snap(0)), Some(snap(1)));
        assert_eq!(h.redo(snap(1)), Some(snap(2)));
        assert_eq!(h.redo(snap(2)), None);
    }

    #[test]
    fn new_edit_clears_redo_branch() {
        let mut h = History::default();
        h.record(snap(0));
        assert!(h.undo(snap(1)).is_some());
        assert!(h.can_redo());
        h.record(snap(0));
        assert!(!h.can_redo());
    }

    #[test]
    fn history_is_capped() {
        let mut h = History::default();
        for i in 0..(HISTORY_LIMIT + 25) {
            h.record(snap(i % 3));
        }
        assert_eq!(h.undo.len(), HISTORY_LIMIT);
    }

    #[test]
    fn text_bounds_grow_with_length_and_font() {
        let short = Shape::Text {
            x: 0.1,
            y: 0.1,
            text: "hi".into(),
            color: "#000".into(),
            font_size: 20.0,
        };
        let long = Shape::Text {
            x: 0.1,
            y: 0.1,
            text: "a much longer annotation".into(),
            color: "#000".into(),
            font_size: 20.0,
        };
        let (_, _, sw, sh) = shape_bounds(&short).unwrap();
        let (_, _, lw, lh) = shape_bounds(&long).unwrap();
        assert!(lw > sw * 5.0);
        assert!(approx(sh, lh));
        // Clicking near the end of long text picks it.
        assert_eq!(
            pick_shape_at(&[long], 0.1 + lw * 0.95, 0.1 + lh / 2.0),
            Some(0)
        );
    }

    #[test]
    fn text_corner_resize_scales_font_uniformly() {
        let text = Shape::Text {
            x: 0.1,
            y: 0.1,
            text: "resize me".into(),
            color: "#000".into(),
            font_size: 20.0,
        };
        assert_eq!(shape_handles(&text).len(), 4);
        let (x, y, w, h) = shape_bounds(&text).unwrap();
        let Shape::Text {
            x: nx,
            y: ny,
            font_size,
            ..
        } = resize_shape(&text, Handle::Se, (x + w * 2.0, y + h * 2.0))
        else {
            panic!("not text")
        };
        assert!(approx(nx, x) && approx(ny, y));
        assert!((font_size - 40.0).abs() < 1e-6);
        // Never shrinks below the readable minimum.
        let Shape::Text { font_size, .. } = resize_shape(&text, Handle::Se, (x, y)) else {
            panic!("not text")
        };
        assert!(approx(font_size, MIN_FONT_PX));
    }
}
