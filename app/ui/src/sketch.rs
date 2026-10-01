//! Experimental AI sketching: describe a sketch (or a change to the current
//! one) in words; the host asks the `claude` CLI for add/update/delete ops
//! in the canvas's annotation JSON, and we apply them here as one undoable
//! edit. Everything the AI produces is ordinary canvas content afterwards —
//! select, move, resize, recolor, undo/redo all work on it.

use std::collections::{BTreeMap, HashSet};

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::Value;
use web_sys::{HtmlTextAreaElement, KeyboardEvent};

use crate::canvas::{
    text_extent, CanvasController, Shape, MAX_FONT_PX, MIN_FONT_PX, MIN_SHAPE_SIZE,
};
use crate::interop;
use crate::types::{SketchOp, SketchRequest};

/// Generous upper bounds on what a single AI response may add.
const MAX_ADDED_SHAPES: usize = 200;
const MAX_PATH_POINTS: usize = 200;
const MAX_TEXT_CHARS: usize = 200;
const DEFAULT_COLOR: &str = "#0f172a";

#[derive(Debug, Default, PartialEq)]
pub struct SketchApplied {
    pub shapes: Vec<Shape>,
    pub added: usize,
    pub updated: usize,
    pub deleted: usize,
    pub skipped: usize,
}

/// Apply AI ops to `base`. Ids are indices into `base`; ops naming unknown
/// ids, unparseable shapes, and image adds are skipped (counted). Every
/// resulting shape is clamped onto the canvas.
pub fn apply_sketch_ops(base: &[Shape], ops: &[SketchOp]) -> SketchApplied {
    let mut out = SketchApplied::default();
    let mut deleted: HashSet<usize> = HashSet::new();
    // Merge all patches per id first so repeated updates compose.
    let mut patches: BTreeMap<usize, serde_json::Map<String, Value>> = BTreeMap::new();
    let mut adds: Vec<Shape> = Vec::new();

    for op in ops {
        match (op.op.as_str(), op.id, op.shape.as_ref()) {
            ("delete", Some(id), _) if id < base.len() => {
                deleted.insert(id);
            }
            ("update", Some(id), Some(Value::Object(patch))) if id < base.len() => {
                let entry = patches.entry(id).or_default();
                for (k, v) in patch {
                    // The model may restyle or move a shape, not change its kind.
                    if k != "type" && k != "id" {
                        entry.insert(k.clone(), v.clone());
                    }
                }
            }
            ("add", _, Some(value @ Value::Object(_))) if adds.len() < MAX_ADDED_SHAPES => {
                match parse_generated(value, None) {
                    Some(shape) => adds.push(shape),
                    None => out.skipped += 1,
                }
            }
            _ => out.skipped += 1,
        }
    }

    for (idx, shape) in base.iter().enumerate() {
        if deleted.contains(&idx) {
            out.deleted += 1;
            continue;
        }
        let Some(patch) = patches.get(&idx) else {
            out.shapes.push(shape.clone());
            continue;
        };
        let mut merged = shape.to_json();
        if let Some(obj) = merged.as_object_mut() {
            for (k, v) in patch {
                obj.insert(k.clone(), v.clone());
            }
        }
        let image_url = match shape {
            Shape::Image { data_url, .. } => Some(&**data_url),
            _ => None,
        };
        match parse_generated(&merged, image_url) {
            Some(next) => {
                if next != *shape {
                    out.updated += 1;
                }
                out.shapes.push(next);
            }
            None => {
                // Keep the original rather than lose user content to a bad patch.
                out.skipped += 1;
                out.shapes.push(shape.clone());
            }
        }
    }

    out.added = adds.len();
    out.shapes.extend(adds);
    out
}

/// Parse one AI shape: resolve `anchor: "center"` text against real text
/// metrics, then sanitise. Images can only come from existing shapes.
fn parse_generated(value: &Value, image_url: Option<&str>) -> Option<Shape> {
    if value.get("type").and_then(Value::as_str) == Some("image") && image_url.is_none() {
        return None;
    }
    let centered = value.get("anchor").and_then(Value::as_str) == Some("center");
    let shape = Shape::from_json(value, image_url)?;
    let shape = match shape {
        Shape::Text {
            x,
            y,
            text,
            color,
            font_size,
        } if centered => {
            let font_size = font_size.clamp(MIN_FONT_PX, MAX_FONT_PX);
            let (w, h) = text_extent(&text, font_size);
            Shape::Text {
                x: x - w / 2.0,
                y: y - h / 2.0,
                text,
                color,
                font_size,
            }
        }
        other => other,
    };
    Some(sanitize(shape))
}

fn clamp01(v: f64) -> f64 {
    if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn clamp_point((x, y): (f64, f64)) -> (f64, f64) {
    (clamp01(x), clamp01(y))
}

/// Keep a box inside the canvas with a non-degenerate size.
fn clamp_box(x: f64, y: f64, w: f64, h: f64) -> (f64, f64, f64, f64) {
    let w = if w.is_finite() {
        w.abs()
    } else {
        MIN_SHAPE_SIZE
    };
    let h = if h.is_finite() {
        h.abs()
    } else {
        MIN_SHAPE_SIZE
    };
    let w = w.clamp(MIN_SHAPE_SIZE, 1.0);
    let h = h.clamp(MIN_SHAPE_SIZE, 1.0);
    (clamp01(x).min(1.0 - w), clamp01(y).min(1.0 - h), w, h)
}

fn clean_color(c: String) -> String {
    let hex = c.trim();
    let ok = hex.starts_with('#')
        && matches!(hex.len(), 4 | 7)
        && hex[1..].chars().all(|ch| ch.is_ascii_hexdigit());
    if ok {
        hex.to_string()
    } else {
        DEFAULT_COLOR.to_string()
    }
}

fn clean_width(w: f64) -> f64 {
    if w.is_finite() {
        w.clamp(1.0, 12.0)
    } else {
        3.0
    }
}

fn sanitize(shape: Shape) -> Shape {
    match shape {
        Shape::Path {
            points,
            color,
            width,
        } => {
            let step = points.len().div_ceil(MAX_PATH_POINTS).max(1);
            Shape::Path {
                points: points.into_iter().step_by(step).map(clamp_point).collect(),
                color: clean_color(color),
                width: clean_width(width),
            }
        }
        Shape::Line {
            from,
            to,
            color,
            width,
        } => Shape::Line {
            from: clamp_point(from),
            to: clamp_point(to),
            color: clean_color(color),
            width: clean_width(width),
        },
        Shape::Arrow {
            from,
            to,
            color,
            width,
        } => Shape::Arrow {
            from: clamp_point(from),
            to: clamp_point(to),
            color: clean_color(color),
            width: clean_width(width),
        },
        Shape::Rect {
            x,
            y,
            w,
            h,
            color,
            width,
        } => {
            let (x, y, w, h) = clamp_box(x, y, w, h);
            Shape::Rect {
                x,
                y,
                w,
                h,
                color: clean_color(color),
                width: clean_width(width),
            }
        }
        Shape::Ellipse {
            x,
            y,
            w,
            h,
            color,
            width,
        } => {
            let (x, y, w, h) = clamp_box(x, y, w, h);
            Shape::Ellipse {
                x,
                y,
                w,
                h,
                color: clean_color(color),
                width: clean_width(width),
            }
        }
        Shape::Highlight { x, y, w, h, color } => {
            let (x, y, w, h) = clamp_box(x, y, w, h);
            Shape::Highlight {
                x,
                y,
                w,
                h,
                color: clean_color(color),
            }
        }
        Shape::Text {
            x,
            y,
            text,
            color,
            font_size,
        } => Shape::Text {
            x: clamp01(x),
            y: clamp01(y),
            text: text.chars().take(MAX_TEXT_CHARS).collect(),
            color: clean_color(color),
            font_size: if font_size.is_finite() {
                font_size.clamp(MIN_FONT_PX, MAX_FONT_PX)
            } else {
                16.0
            },
        },
        Shape::Image {
            x,
            y,
            w,
            h,
            data_url,
        } => {
            let (x, y, w, h) = clamp_box(x, y, w, h);
            Shape::Image {
                x,
                y,
                w,
                h,
                data_url,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Prompt popover
// ---------------------------------------------------------------------------

/// State that must outlive the popover (a request keeps running if the
/// popover is closed). Owned by the toolbar.
#[derive(Clone, Copy)]
pub struct SketchState {
    prompt: RwSignal<String>,
    status: RwSignal<Option<Result<String, String>>>,
    /// Bumped on every request and on cancel; a response whose token no
    /// longer matches is discarded.
    request_token: StoredValue<u64>,
}

impl SketchState {
    pub fn new() -> Self {
        Self {
            prompt: RwSignal::new(String::new()),
            status: RwSignal::new(None),
            request_token: StoredValue::new(0),
        }
    }
}

fn scope_label(ctrl: &CanvasController) -> &'static str {
    if ctrl.shapes.with(Vec::is_empty) {
        "Describe a sketch to draw"
    } else if ctrl.selected_idx.get().is_some() {
        "Describe a change to the selected shape"
    } else {
        "Describe a change to this sketch"
    }
}

fn run_generation(ctrl: CanvasController, state: SketchState) {
    let prompt = state.prompt.get_untracked().trim().to_string();
    if prompt.is_empty() || ctrl.sketch_busy.get_untracked() {
        return;
    }
    let base = ctrl.shapes.get_untracked();
    let (cw, ch) = ctrl.canvas_size();
    let request = SketchRequest {
        prompt,
        shapes: CanvasController::annotations_for_shapes(&base),
        selected: ctrl.selected_idx.get_untracked().into_iter().collect(),
        canvas_width: cw,
        canvas_height: ch,
    };
    let token = state.request_token.get_value().wrapping_add(1);
    state.request_token.set_value(token);
    ctrl.sketch_busy.set(true);
    state.status.set(None);

    spawn_local(async move {
        let result = interop::generate_sketch(&request).await;
        // Cancelled (or superseded) while in flight: drop the response.
        if state.request_token.try_get_value() != Some(token) {
            return;
        }
        ctrl.sketch_busy.set(false);
        let status = match result {
            Ok(resp) => {
                let applied = apply_sketch_ops(&base, &resp.ops);
                if applied.added + applied.updated + applied.deleted == 0 {
                    Err("The AI returned no usable changes — try rephrasing.".to_string())
                } else {
                    let summary = format!(
                        "Added {} · changed {} · removed {}{} — {} in {:.1}s. Undo with ⌘Z.",
                        applied.added,
                        applied.updated,
                        applied.deleted,
                        if applied.skipped > 0 {
                            format!(" · skipped {}", applied.skipped)
                        } else {
                            String::new()
                        },
                        resp.model,
                        resp.duration_ms as f64 / 1000.0,
                    );
                    ctrl.apply_generated(&base, applied.shapes).map(|_| summary)
                }
            }
            Err(err) => Err(err),
        };
        if status.is_ok() {
            state.prompt.try_set(String::new());
        }
        state.status.try_set(Some(status));
    });
}

fn cancel_generation(ctrl: &CanvasController, state: SketchState) {
    if !ctrl.sketch_busy.get_untracked() {
        return;
    }
    state
        .request_token
        .set_value(state.request_token.get_value().wrapping_add(1));
    ctrl.sketch_busy.set(false);
    state.status.set(Some(Err(
        "Cancelled — the canvas was left unchanged.".into()
    )));
}

#[component]
pub fn SketchPrompt<F>(controller: CanvasController, state: SketchState, close: F) -> impl IntoView
where
    F: Fn() + Clone + 'static,
{
    let busy = controller.sketch_busy;
    let label = {
        let ctrl = controller.clone();
        move || scope_label(&ctrl)
    };
    let generate = {
        let ctrl = controller.clone();
        move || run_generation(ctrl.clone(), state)
    };
    let cancel = {
        let ctrl = controller.clone();
        move || cancel_generation(&ctrl, state)
    };
    let on_keydown = {
        let generate = generate.clone();
        let cancel = cancel.clone();
        let close = close.clone();
        move |ev: KeyboardEvent| {
            if ev.key() == "Enter" && (ev.meta_key() || ev.ctrl_key()) {
                ev.prevent_default();
                generate();
            } else if ev.key() == "Escape" {
                ev.prevent_default();
                if busy.get_untracked() {
                    cancel();
                } else {
                    close();
                }
            }
        }
    };

    view! {
        <div class="popover popover-sketch" role="dialog" aria-label="Describe a sketch with AI">
            <div class="sketch-label">{label}</div>
            <textarea
                class="sketch-input"
                rows="3"
                placeholder="e.g. A login screen with email + password fields, a big Sign in button and a 'Forgot password?' link"
                prop:value=move || state.prompt.get()
                disabled=move || busy.get()
                autofocus=true
                on:input=move |ev| {
                    let el = event_target::<HtmlTextAreaElement>(&ev);
                    state.prompt.set(el.value());
                }
                on:keydown=on_keydown
            ></textarea>
            <div class="sketch-actions">
                <span class="sketch-hint">
                    {move || if busy.get() { "Generating… Esc to cancel" } else { "⌘↵ to generate" }}
                </span>
                {
                    let generate = generate.clone();
                    let cancel = cancel.clone();
                    move || {
                        if busy.get() {
                            let cancel = cancel.clone();
                            view! {
                                <button class="btn btn-ghost sketch-btn" on:click=move |_| cancel()>
                                    "Cancel"
                                </button>
                            }
                            .into_any()
                        } else {
                            let generate = generate.clone();
                            view! {
                                <button
                                    class="btn btn-primary sketch-btn"
                                    disabled=move || state.prompt.with(|p| p.trim().is_empty())
                                    on:click=move |_| generate()
                                >
                                    "✦ Generate"
                                </button>
                            }
                            .into_any()
                        }
                    }
                }
            </div>
            {move || match state.status.get() {
                Some(Ok(msg)) => view! { <div class="sketch-status ok">{msg}</div> }.into_any(),
                Some(Err(msg)) => view! { <div class="sketch-status error">{msg}</div> }.into_any(),
                None => view! { <span></span> }.into_any(),
            }}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn op(kind: &str, id: Option<usize>, shape: Option<Value>) -> SketchOp {
        SketchOp {
            op: kind.into(),
            id,
            shape,
        }
    }

    fn rect(x: f64) -> Shape {
        Shape::Rect {
            x,
            y: 0.1,
            w: 0.1,
            h: 0.1,
            color: "#0f172a".into(),
            width: 3.0,
        }
    }

    #[test]
    fn adds_updates_and_deletes_against_base_ids() {
        let base = vec![rect(0.1), rect(0.3), rect(0.5)];
        let ops = vec![
            op("delete", Some(0), None),
            op("update", Some(1), Some(json!({"color": "#ef4444"}))),
            op(
                "add",
                None,
                Some(
                    json!({"type": "ellipse", "x": 0.7, "y": 0.7, "w": 0.1, "h": 0.1, "color": "#2563eb", "width": 3}),
                ),
            ),
        ];
        let applied = apply_sketch_ops(&base, &ops);
        assert_eq!(
            (
                applied.added,
                applied.updated,
                applied.deleted,
                applied.skipped
            ),
            (1, 1, 1, 0)
        );
        assert_eq!(applied.shapes.len(), 3);
        assert!(matches!(&applied.shapes[0], Shape::Rect { color, .. } if color == "#ef4444"));
        assert_eq!(applied.shapes[1], rect(0.5));
        assert!(matches!(applied.shapes[2], Shape::Ellipse { .. }));
    }

    #[test]
    fn unknown_ids_bad_shapes_and_image_adds_are_skipped() {
        let base = vec![rect(0.1)];
        let ops = vec![
            op("delete", Some(9), None),
            op("update", Some(9), Some(json!({"color": "#ef4444"}))),
            op("add", None, Some(json!({"type": "rect", "x": 0.1}))),
            op(
                "add",
                None,
                Some(json!({"type": "image", "x": 0.1, "y": 0.1, "w": 0.1, "h": 0.1})),
            ),
            op("teleport", Some(0), None),
        ];
        let applied = apply_sketch_ops(&base, &ops);
        assert_eq!(applied.skipped, 5);
        assert_eq!(applied.shapes, base);
    }

    #[test]
    fn update_cannot_change_shape_kind_and_bad_patch_keeps_original() {
        let base = vec![rect(0.1)];
        let applied = apply_sketch_ops(
            &base,
            &[op(
                "update",
                Some(0),
                Some(json!({"type": "ellipse", "x": 0.2})),
            )],
        );
        assert!(matches!(applied.shapes[0], Shape::Rect { x, .. } if (x - 0.2).abs() < 1e-9));

        let applied = apply_sketch_ops(&base, &[op("update", Some(0), Some(json!({"x": "left"})))]);
        assert_eq!(applied.shapes, base);
        assert_eq!(applied.skipped, 1);
    }

    #[test]
    fn generated_shapes_are_clamped_and_cleaned() {
        let applied = apply_sketch_ops(
            &[],
            &[op(
                "add",
                None,
                Some(
                    json!({"type": "rect", "x": 0.95, "y": -0.2, "w": 0.3, "h": 0.1, "color": "red", "width": 40}),
                ),
            )],
        );
        let Shape::Rect {
            x,
            y,
            w,
            color,
            width,
            ..
        } = &applied.shapes[0]
        else {
            panic!("not a rect")
        };
        assert!(x + w <= 1.0 + 1e-9);
        assert_eq!(*y, 0.0);
        assert_eq!(color, DEFAULT_COLOR);
        assert_eq!(*width, 12.0);
    }

    #[test]
    fn centered_text_is_placed_by_its_middle() {
        let applied = apply_sketch_ops(
            &[],
            &[op(
                "add",
                None,
                Some(
                    json!({"type": "text", "x": 0.5, "y": 0.5, "text": "Start", "fontSize": 20, "color": "#2563eb", "anchor": "center"}),
                ),
            )],
        );
        let shape = &applied.shapes[0];
        let Shape::Text {
            x,
            y,
            text,
            font_size,
            ..
        } = shape
        else {
            panic!("not text")
        };
        let (w, h) = text_extent(text, *font_size);
        assert!((x + w / 2.0 - 0.5).abs() < 1e-9);
        assert!((y + h / 2.0 - 0.5).abs() < 1e-9);
    }

    #[test]
    fn repeated_updates_to_one_shape_compose() {
        let applied = apply_sketch_ops(
            &[rect(0.1)],
            &[
                op("update", Some(0), Some(json!({"color": "#ef4444"}))),
                op("update", Some(0), Some(json!({"width": 6}))),
            ],
        );
        assert!(matches!(
            &applied.shapes[0],
            Shape::Rect { color, width, .. } if color == "#ef4444" && *width == 6.0
        ));
        assert_eq!(applied.updated, 1);
    }
}
