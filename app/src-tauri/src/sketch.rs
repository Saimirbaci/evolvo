//! AI sketch generation for the Canvas overlay (experimental).
//!
//! The user describes a sketch (or a change to the current one) in words;
//! we ask the `claude` CLI for structured output and hand back a list of
//! add / update / delete ops expressed in the canvas's own annotation JSON
//! (`Shape::to_json` on the UI side). The UI validates every shape and
//! applies the ops as a single undoable edit, so AI output is ordinary,
//! editable canvas content.
//!
//! Safety envelope: the CLI runs with **no tools** (`--tools ""`), in its own
//! process group with a hard timeout, and never sees anything but the prompt
//! and the shape JSON. The prompt travels over stdin (shape lists with long
//! freehand strokes can exceed ARG_MAX), stdout goes to a temp file (a full
//! pipe would deadlock `run_with_timeout`'s poll loop).

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::enriched_path;
use crate::proc::{run_with_timeout, WaitOutcome};

/// Opus draws noticeably better layouts from scratch; edits are equally
/// good on Sonnet at roughly half the cost (see the model comparison spike).
const DEFAULT_GENERATE_MODEL: &str = "opus";
const DEFAULT_EDIT_MODEL: &str = "sonnet";
const SKETCH_TIMEOUT: Duration = Duration::from_secs(180);
const MAX_PROMPT_CHARS: usize = 4_000;
const MAX_INPUT_SHAPES: usize = 300;
const MAX_OPS: usize = 200;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SketchRequest {
    pub prompt: String,
    /// Current canvas annotations (`Shape::to_json`), ids = array index.
    #[serde(default)]
    pub shapes: Vec<Value>,
    /// Ids the user has selected; the edit is scoped to them when present.
    #[serde(default)]
    pub selected: Vec<usize>,
    /// Canvas size in CSS px, so the model reasons about real proportions.
    #[serde(default)]
    pub canvas_width: f64,
    #[serde(default)]
    pub canvas_height: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SketchOp {
    pub op: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SketchResponse {
    pub ops: Vec<SketchOp>,
    pub model: String,
    pub duration_ms: u64,
}

/// Fully-resolved CLI invocation, separated from execution so tests can
/// assert on it and inject a fake runner.
#[derive(Debug, Clone)]
pub struct SketchInvocation {
    pub model: String,
    pub system_prompt: String,
    pub user_prompt: String,
    pub schema: String,
}

pub fn generate_sketch(req: &SketchRequest) -> Result<SketchResponse, String> {
    generate_sketch_with(req, run_claude)
}

/// Core with an injectable runner (returns the CLI's raw `--output-format
/// json` stdout).
pub fn generate_sketch_with(
    req: &SketchRequest,
    runner: impl FnOnce(&SketchInvocation) -> Result<String, String>,
) -> Result<SketchResponse, String> {
    let invocation = build_invocation(req)?;
    let started = Instant::now();
    let stdout = runner(&invocation)?;
    let ops = parse_cli_output(&stdout)?;
    Ok(SketchResponse {
        ops,
        model: invocation.model,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

pub fn build_invocation(req: &SketchRequest) -> Result<SketchInvocation, String> {
    let prompt = req.prompt.trim();
    if prompt.is_empty() {
        return Err("Describe what to draw first.".into());
    }
    if prompt.chars().count() > MAX_PROMPT_CHARS {
        return Err(format!(
            "Description is too long (max {MAX_PROMPT_CHARS} characters)."
        ));
    }
    if req.shapes.len() > MAX_INPUT_SHAPES {
        return Err(format!(
            "The canvas has too many shapes for AI editing (max {MAX_INPUT_SHAPES})."
        ));
    }

    let editing = !req.shapes.is_empty();
    let model = if editing {
        model_from_env("EVOLVO_SKETCH_MODEL_EDIT", DEFAULT_EDIT_MODEL)
    } else {
        model_from_env("EVOLVO_SKETCH_MODEL_GENERATE", DEFAULT_GENERATE_MODEL)
    };

    let (cw, ch) = if req.canvas_width > 0.0 && req.canvas_height > 0.0 {
        (req.canvas_width.round(), req.canvas_height.round())
    } else {
        (1280.0, 800.0)
    };

    let mut user_prompt = String::new();
    if editing {
        let with_ids: Vec<Value> = req
            .shapes
            .iter()
            .enumerate()
            .map(|(id, shape)| {
                let mut shape = round_numbers(shape.clone());
                if let Some(obj) = shape.as_object_mut() {
                    obj.insert("id".into(), id.into());
                }
                shape
            })
            .collect();
        user_prompt.push_str("Current canvas shapes (with ids):\n");
        user_prompt.push_str(&Value::Array(with_ids).to_string());
        user_prompt.push_str("\n\n");
        let selected: Vec<usize> = req
            .selected
            .iter()
            .copied()
            .filter(|id| *id < req.shapes.len())
            .collect();
        if !selected.is_empty() {
            user_prompt.push_str(&format!(
                "The user has selected shape ids {selected:?}. Apply the change to those \
                 shapes; only touch others if the request clearly requires it.\n\n"
            ));
        }
        user_prompt.push_str("Change request:\n");
    } else {
        user_prompt.push_str("The canvas is empty. Sketch this:\n");
    }
    user_prompt.push_str(prompt);

    Ok(SketchInvocation {
        model,
        system_prompt: system_prompt(cw, ch, editing),
        user_prompt,
        schema: ops_schema().to_string(),
    })
}

fn model_from_env(var: &str, default: &str) -> String {
    std::env::var(var)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn system_prompt(cw: f64, ch: f64, editing: bool) -> String {
    let mut s = format!(
        "You draw wireframe sketches on an annotation canvas of {cw}x{ch} px. \
All coordinates are ratios 0..1 of the canvas (x right, y down).\n\
Shape types:\n\
- rect / ellipse / highlight: x, y (top-left), w, h. highlight is a translucent fill for emphasis.\n\
- line / arrow: from [x,y], to [x,y].\n\
- path: points [[x,y], ...] for freehand marks or simple icons (keep under 40 points).\n\
- text: x, y, text, fontSize (px, 12-48). Single line. Set \"anchor\": \"center\" to make x,y \
the CENTER of the text — use this for every label inside a button, circle, tag or any \
centered element, placing x,y at the container's center. Otherwise x,y is the top-left.\n\
Every shape has color (hex like #0f172a). Stroked shapes have width: 1.5, 3 or 6.\n\
Style: #0f172a for structure, #2563eb for primary actions, #ef4444 for emphasis; other \
hex colors for tags are fine. Align to a consistent grid with even margins, keep text inside \
its containers, never overlap unrelated elements, and keep everything inside the canvas.\n"
    );
    if editing {
        s.push_str(
            "You are editing an existing sketch. Return the MINIMAL list of ops: \
\"update\" (id + only the changed fields in shape), \"delete\" (id), \"add\" (full shape). \
Never re-add or rewrite shapes that do not need to change, and only use ids that exist.",
        );
    } else {
        s.push_str(
            "The canvas is empty: return one \"add\" op (with a full shape) per shape. \
A rich but clean wireframe usually needs 15-60 shapes.",
        );
    }
    s
}

fn ops_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "required": ["ops"],
        "additionalProperties": false,
        "properties": {
            "ops": {
                "type": "array",
                "maxItems": MAX_OPS,
                "items": {
                    "type": "object",
                    "required": ["op"],
                    "properties": {
                        "op": { "enum": ["add", "update", "delete"] },
                        "id": { "type": "integer", "minimum": 0 },
                        "shape": {
                            "type": "object",
                            "properties": {
                                "type": { "enum": ["rect", "ellipse", "line", "arrow", "text", "highlight", "path"] },
                                "x": { "type": "number" }, "y": { "type": "number" },
                                "w": { "type": "number" }, "h": { "type": "number" },
                                "from": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2 },
                                "to": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2 },
                                "points": { "type": "array", "items": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2 } },
                                "text": { "type": "string" },
                                "fontSize": { "type": "number" },
                                "anchor": { "enum": ["topLeft", "center"] },
                                "color": { "type": "string" },
                                "width": { "type": "number" }
                            }
                        }
                    }
                }
            }
        }
    })
}

/// Round floats to 4 decimals — plenty for a ratio canvas, and it keeps
/// long freehand paths from bloating the prompt.
fn round_numbers(value: Value) -> Value {
    match value {
        Value::Number(n) => match n.as_f64() {
            Some(f) if n.is_f64() => {
                serde_json::Number::from_f64((f * 10_000.0).round() / 10_000.0)
                    .map(Value::Number)
                    .unwrap_or(Value::Number(n))
            }
            _ => Value::Number(n),
        },
        Value::Array(items) => Value::Array(items.into_iter().map(round_numbers).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, round_numbers(v)))
                .collect(),
        ),
        other => other,
    }
}

/// Extract ops from `claude -p --output-format json` stdout. Ops with an
/// unknown verb or missing fields are dropped here; shape validation is the
/// UI's job (it owns the `Shape` type).
pub fn parse_cli_output(stdout: &str) -> Result<Vec<SketchOp>, String> {
    let envelope: Value = serde_json::from_str(stdout.trim())
        .map_err(|e| format!("AI response was not valid JSON: {e}"))?;
    if envelope.get("is_error").and_then(Value::as_bool) == Some(true) {
        let msg = envelope
            .get("result")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        return Err(format!("AI request failed: {}", truncate(msg, 300)));
    }
    let ops = envelope
        .get("structured_output")
        .and_then(|so| so.get("ops"))
        .and_then(Value::as_array)
        .ok_or("AI response had no sketch ops.")?;
    Ok(ops
        .iter()
        .take(MAX_OPS)
        .filter_map(|raw| serde_json::from_value::<SketchOp>(raw.clone()).ok())
        .filter(|op| match op.op.as_str() {
            "add" => op.shape.as_ref().is_some_and(Value::is_object),
            "update" => op.id.is_some() && op.shape.as_ref().is_some_and(Value::is_object),
            "delete" => op.id.is_some(),
            _ => false,
        })
        .collect())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

fn run_claude(inv: &SketchInvocation) -> Result<String, String> {
    let mut stdin_file =
        tempfile::NamedTempFile::new().map_err(|e| format!("temp file error: {e}"))?;
    stdin_file
        .write_all(inv.user_prompt.as_bytes())
        .map_err(|e| format!("temp file error: {e}"))?;
    let stdout_file =
        tempfile::NamedTempFile::new().map_err(|e| format!("temp file error: {e}"))?;
    let stdin = stdin_file
        .reopen()
        .map_err(|e| format!("temp file error: {e}"))?;
    let stdout = stdout_file
        .reopen()
        .map_err(|e| format!("temp file error: {e}"))?;

    let mut cmd = Command::new("claude");
    cmd.arg("-p")
        .arg("--model")
        .arg(&inv.model)
        .arg("--system-prompt")
        .arg(&inv.system_prompt)
        .arg("--json-schema")
        .arg(&inv.schema)
        .arg("--output-format")
        .arg("json")
        .arg("--tools")
        .arg("")
        .env("PATH", enriched_path())
        // Same auth policy as lineage runs: prefer the subscription login
        // over a possibly-empty API key.
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::null());

    match run_with_timeout(&mut cmd, SKETCH_TIMEOUT) {
        Ok(WaitOutcome::Exited(_)) => {}
        Ok(WaitOutcome::TimedOut) => {
            return Err(format!(
                "AI sketch timed out after {}s.",
                SKETCH_TIMEOUT.as_secs()
            ))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err("The `claude` CLI is not installed or not on PATH.".into())
        }
        Err(e) => return Err(format!("Could not start `claude`: {e}")),
    }
    // Exit status is not trusted on its own: the JSON envelope carries
    // `is_error` and the real message, which parse_cli_output surfaces.
    std::fs::read_to_string(stdout_file.path()).map_err(|e| format!("read AI output: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn req(prompt: &str, shapes: Vec<Value>) -> SketchRequest {
        SketchRequest {
            prompt: prompt.into(),
            shapes,
            selected: vec![],
            canvas_width: 1440.0,
            canvas_height: 900.0,
        }
    }

    #[test]
    fn empty_canvas_uses_generate_model_and_add_only_instructions() {
        let inv = build_invocation(&req("a pomodoro timer", vec![])).unwrap();
        assert_eq!(inv.model, DEFAULT_GENERATE_MODEL);
        assert!(inv.system_prompt.contains("1440x900"));
        assert!(inv.system_prompt.contains("The canvas is empty"));
        assert!(inv.user_prompt.ends_with("a pomodoro timer"));
        let schema: Value = serde_json::from_str(&inv.schema).unwrap();
        assert_eq!(schema["properties"]["ops"]["maxItems"], MAX_OPS);
    }

    #[test]
    fn edit_includes_ids_selection_and_rounds_coordinates() {
        let shapes = vec![
            json!({"type": "rect", "x": 0.123456789, "y": 0.1, "w": 0.2, "h": 0.2, "color": "#000000", "width": 3.0}),
            json!({"type": "text", "x": 0.5, "y": 0.5, "text": "hi", "color": "#000000", "fontSize": 20.0}),
        ];
        let mut r = req("make the box red", shapes);
        r.selected = vec![0, 99];
        let inv = build_invocation(&r).unwrap();
        assert_eq!(inv.model, DEFAULT_EDIT_MODEL);
        assert!(inv.user_prompt.contains("\"id\":0"));
        assert!(inv.user_prompt.contains("\"id\":1"));
        assert!(inv.user_prompt.contains("0.1235"));
        assert!(!inv.user_prompt.contains("0.123456789"));
        // Out-of-range selection ids are dropped.
        assert!(inv.user_prompt.contains("selected shape ids [0]"));
        assert!(inv.system_prompt.contains("MINIMAL"));
    }

    #[test]
    fn rejects_empty_and_oversized_prompts() {
        assert!(build_invocation(&req("   ", vec![])).is_err());
        let long = "x".repeat(MAX_PROMPT_CHARS + 1);
        assert!(build_invocation(&req(&long, vec![])).is_err());
    }

    #[test]
    fn parse_keeps_well_formed_ops_and_drops_the_rest() {
        let stdout = json!({
            "type": "result",
            "is_error": false,
            "structured_output": {"ops": [
                {"op": "add", "shape": {"type": "rect"}},
                {"op": "add"},
                {"op": "update", "id": 2, "shape": {"color": "#ef4444"}},
                {"op": "update", "shape": {"color": "#ef4444"}},
                {"op": "delete", "id": 1},
                {"op": "explode", "id": 1}
            ]}
        })
        .to_string();
        let ops = parse_cli_output(&stdout).unwrap();
        let kinds: Vec<_> = ops.iter().map(|o| (o.op.as_str(), o.id)).collect();
        assert_eq!(
            kinds,
            vec![("add", None), ("update", Some(2)), ("delete", Some(1))]
        );
    }

    #[test]
    fn parse_surfaces_cli_errors() {
        let err = parse_cli_output(r#"{"is_error": true, "result": "Not logged in"}"#).unwrap_err();
        assert!(err.contains("Not logged in"));
        assert!(parse_cli_output("not json").is_err());
        assert!(parse_cli_output(r#"{"is_error": false}"#).is_err());
    }

    #[test]
    fn generate_with_injected_runner_returns_ops() {
        let resp = generate_sketch_with(&req("a box", vec![]), |inv| {
            assert!(inv.user_prompt.contains("a box"));
            Ok(json!({"is_error": false, "structured_output": {"ops": [
                {"op": "add", "shape": {"type": "rect", "x": 0.1, "y": 0.1, "w": 0.2, "h": 0.2, "color": "#000000", "width": 3}}
            ]}})
            .to_string())
        })
        .unwrap();
        assert_eq!(resp.ops.len(), 1);
        assert_eq!(resp.model, DEFAULT_GENERATE_MODEL);
    }

    #[test]
    fn runner_errors_propagate() {
        let err = generate_sketch_with(&req("a box", vec![]), |_| Err("boom".into())).unwrap_err();
        assert_eq!(err, "boom");
    }
}
