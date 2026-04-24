//! `IterationPlan` — the machine-readable contract that drives the multi-stage
//! NewApp pipeline.
//!
//! Philosophy (read before editing):
//!
//! - **The canvas PNG is the source of truth.** No stage ever consumes a
//!   prose summary of the drawing produced by another stage. This module
//!   therefore carries *pointers and mechanical facts* (bboxes, stroke
//!   colors, stroke counts) extracted deterministically from the
//!   annotations JSON — never interpretations.
//! - **Forward-compatible.** Every stage section is `#[serde(default)]` so
//!   a partially-filled plan (planner 1 done, planner 2 pending) round-trips
//!   cleanly and validators can tell which stages are still empty.
//! - **Every planner writes its own section; readers read the current
//!   state.** There are no patches or merge-logic — drift surfaces as a
//!   validator rejection, not a silent edit.
//!
//! On-disk location: `<job_workspace>/plan.json`, written by the runner
//! before the first planner session starts and mutated by each subsequent
//! stage.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::store::StoreError;

pub const PLAN_FILENAME: &str = "plan.json";
pub const PLAN_SCHEMA_VERSION: u32 = 1;

/// State of the plan file in terms of which stages have produced content.
/// Written by each stage's planner *and* by the validator when a stage is
/// accepted. The runner uses it to decide what to dispatch next.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PlanStage {
    #[default]
    Seeded,
    BackendPlanned,
    BackendImplemented,
    FrontendPlanned,
    FrontendImplemented,
    E2EPlanned,
    E2EImplemented,
    Completed,
    Failed,
}

impl PlanStage {
    pub fn label(self) -> &'static str {
        match self {
            Self::Seeded => "seeded",
            Self::BackendPlanned => "backend planned",
            Self::BackendImplemented => "backend implemented",
            Self::FrontendPlanned => "frontend planned",
            Self::FrontendImplemented => "frontend implemented",
            Self::E2EPlanned => "e2e planned",
            Self::E2EImplemented => "e2e implemented",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct IterationPlan {
    /// Schema version. Bump when incompatible changes land. Missing / older
    /// values are treated as `0` and should be upgraded by the runner.
    #[serde(default)]
    pub schema_version: u32,
    pub app: AppIdentity,
    #[serde(default)]
    pub stage: PlanStage,
    /// Pointer to the canvas PNG (absolute path on the host, inside the
    /// job's `inputs/` directory) plus mechanically-extracted regions.
    /// Every subsequent stage re-reads the PNG; the regions are only an
    /// index, not a substitute for the pixels.
    pub canvas: CanvasReference,
    /// Verbatim user text from the feedback submission. NOT a summary — the
    /// original words so every stage works from the same source.
    #[serde(default)]
    pub user_text: String,
    /// Verbatim voice transcript if the user recorded one. NOT a summary.
    #[serde(default)]
    pub voice_transcript: Option<String>,
    #[serde(default)]
    pub templates: TemplatesSection,
    #[serde(default)]
    pub backend: BackendSection,
    #[serde(default)]
    pub frontend: FrontendSection,
    #[serde(default)]
    pub e2e: E2ESection,
    /// Append-only history: each stage / validator writes a one-line entry
    /// here so every stage after sees what happened upstream without having
    /// to re-read logs. Never rewritten.
    #[serde(default)]
    pub history: Vec<HistoryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct AppIdentity {
    pub name: String,
    #[serde(default)]
    pub one_liner: String,
    #[serde(default)]
    pub domain: String,
    #[serde(default)]
    pub feedback_id: String,
    #[serde(default)]
    pub job_id: String,
    #[serde(default)]
    pub iteration: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct CanvasReference {
    /// Absolute path to the canvas PNG staged under the job's `inputs/`
    /// directory. Every stage that touches user-visible artifacts MUST
    /// attach this file directly to its prompt — never consume a summary.
    #[serde(default)]
    pub png_path: Option<String>,
    /// Absolute path to the raw annotations JSON if one was staged.
    #[serde(default)]
    pub annotations_path: Option<String>,
    /// The annotated page the canvas was drawn over (e.g. "/" or
    /// "/projects/:id"). Preserved verbatim from the feedback record.
    #[serde(default)]
    pub route: String,
    /// Viewport dimensions at draw time (pixels). Lets implementers reason
    /// about layout — e.g. "the arrow points at (312, 480) in a 1440×900
    /// window".
    #[serde(default)]
    pub viewport_width: u32,
    #[serde(default)]
    pub viewport_height: u32,
    /// Total number of stroke primitives in the annotations. Zero = the
    /// user submitted text-only feedback and the canvas PNG is just the
    /// underlying page screenshot.
    #[serde(default)]
    pub stroke_count: u32,
    /// Mechanical region index extracted by `extract_regions` below. Each
    /// region is a cluster of nearby annotations — e.g. all strokes of the
    /// red circle the user drew around the budget field collapse into one
    /// region. Deterministic; no LLM involved.
    #[serde(default)]
    pub regions: Vec<CanvasRegion>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CanvasRegion {
    /// Stable region ID — `R1`, `R2`, … assigned in cluster order. Stage
    /// artifacts reference these IDs so traceability survives re-reads.
    pub id: String,
    /// Axis-aligned bounding box `[x, y, w, h]` in canvas pixel
    /// coordinates (the same space as `viewport_width` / `viewport_height`).
    pub bbox: [f32; 4],
    /// Number of individual stroke / annotation primitives that clustered
    /// into this region.
    pub stroke_count: u32,
    /// Dominant stroke color observed in the cluster (e.g. `#ff0000`).
    /// Useful because users commonly draw red = problem, green = good.
    #[serde(default)]
    pub dominant_color: Option<String>,
    /// Verbatim text labels the user typed *inside* or adjacent to this
    /// cluster (if the annotations carry any). Preserved as-is; never
    /// paraphrased.
    #[serde(default)]
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct TemplatesSection {
    /// List of integration template names to copy in (e.g. `openrouter`,
    /// `sqlite`). Deterministically validated — every listed template must
    /// exist on disk under `templates/integrations/<name>/`.
    #[serde(default)]
    pub use_templates: Vec<String>,
    /// Explicit reason per declined template ("why no auth: app has no
    /// user accounts"). Forces planners to justify omission instead of
    /// silently skipping integrations the shape match implies.
    #[serde(default)]
    pub declined: Vec<DeclinedTemplate>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeclinedTemplate {
    pub name: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct BackendSection {
    #[serde(default)]
    pub entities: Vec<EntityPlan>,
    #[serde(default)]
    pub commands: Vec<CommandPlan>,
    #[serde(default)]
    pub tests: Vec<TestPlan>,
    #[serde(default)]
    pub storage: StoragePlan,
    #[serde(default)]
    pub budget: BackendBudget,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EntityPlan {
    pub name: String,
    pub fields: Vec<FieldPlan>,
    /// Region IDs this entity exists to satisfy. Unsatisfied regions =
    /// validator rejection at the end of the pipeline.
    #[serde(default)]
    pub motivated_by_regions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FieldPlan {
    pub name: String,
    /// Rust type as a string (e.g. `String`, `i64`, `Vec<String>`,
    /// `Option<DateTime<Utc>>`). Validated only at compile time by
    /// `cargo check`, intentionally.
    pub ty: String,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CommandPlan {
    /// Tauri command name in snake_case, matching the `#[tauri::command]`
    /// function. Validator greps `invoke_handler!` for this identifier.
    pub name: String,
    /// Rust type name of the payload struct (e.g. `CreateProjectPayload`)
    /// or `()` when the command takes no arguments.
    pub input: String,
    /// Rust type of the return value inside `Result<T, String>`.
    pub output: String,
    #[serde(default)]
    pub motivated_by_regions: Vec<String>,
    /// One-line description of the *behaviour*, not the implementation. The
    /// implementer decides how; the reviewer checks whether behaviour
    /// matches the region(s).
    #[serde(default)]
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TestPlan {
    /// Exact test function name. Validator greps `#[test] fn <name>` in
    /// the diff.
    pub name: String,
    /// Module path hint (e.g. `store::tests`, `commands::tests`). Not
    /// enforced as strict location — the grep is on the function name.
    #[serde(default)]
    pub module: String,
    pub covers: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum StorageKind {
    #[default]
    JsonPerEntity,
    Sqlite,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct StoragePlan {
    #[serde(default)]
    pub kind: StorageKind,
    /// Directory or file path (relative to `EVOLVO_WORKSPACE_ROOT`) where
    /// the NewApp persists its data. Must live under the workspace root —
    /// validator refuses absolute paths outside it.
    #[serde(default)]
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BackendBudget {
    pub min_entities: u32,
    pub min_commands: u32,
    pub min_tests: u32,
}

impl Default for BackendBudget {
    fn default() -> Self {
        Self {
            min_entities: 1,
            min_commands: 4,
            min_tests: 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct FrontendSection {
    #[serde(default)]
    pub routes: Vec<RoutePlan>,
    #[serde(default)]
    pub components: Vec<ComponentPlan>,
    #[serde(default)]
    pub budget: FrontendBudget,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RoutePlan {
    pub path: String,
    pub component: String,
    #[serde(default)]
    pub uses_commands: Vec<String>,
    #[serde(default)]
    pub motivated_by_regions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ComponentPlan {
    pub name: String,
    #[serde(default)]
    pub module: String,
    #[serde(default)]
    pub uses_commands: Vec<String>,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub motivated_by_regions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FrontendBudget {
    pub min_routes: u32,
    pub min_components: u32,
}

impl Default for FrontendBudget {
    fn default() -> Self {
        Self {
            min_routes: 1,
            min_components: 2,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct E2ESection {
    #[serde(default)]
    pub scenarios: Vec<E2EScenario>,
    #[serde(default)]
    pub persistence_smoke: Option<PersistenceSmoke>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct E2EScenario {
    pub id: String,
    pub title: String,
    /// Ordered, imperative step list ("Click 'New project'", "Type 'Hello'
    /// into the name field", "Press Enter", "Expect the list to contain
    /// 'Hello'"). Implementer executes these verbatim during verification.
    pub steps: Vec<String>,
    #[serde(default)]
    pub motivated_by_regions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PersistenceSmoke {
    /// Entity name the smoke test creates, kills the app, then re-reads.
    pub entity: String,
    /// Directory (under the iteration run's `EVOLVO_WORKSPACE_ROOT`) where
    /// the JSON file should materialise.
    pub expected_directory: String,
}

/// History entry recording one event in a stage's lifecycle (dispatch, write,
/// validation outcome, free-form note). Every field is `#[serde(default)]`
/// because this block is the most-drifted part of the plan when Claude writes
/// it by hand: the golden example shows `{atUnixMs, stage, kind, message}`
/// but sessions routinely produce `{stage, note}` or forget the timestamp.
/// We tolerate any of those shapes so a single sloppy entry never poisons
/// deserialisation of the whole plan — the validators still check *semantic*
/// completeness downstream.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    #[serde(default)]
    pub at_unix_ms: u64,
    #[serde(default)]
    pub stage: String,
    #[serde(default)]
    pub kind: HistoryKind,
    /// Accept `"note"` as an alias so Claude's common drift
    /// (`{"stage": "...", "note": "..."}`) still round-trips cleanly.
    #[serde(default, alias = "note")]
    pub message: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum HistoryKind {
    Started,
    Wrote,
    Validated,
    Rejected,
    #[default]
    Note,
}

/// Read `<job_dir>/plan.json`, returning `Ok(None)` if it's missing.
pub fn load_plan(job_dir: &Path) -> Result<Option<IterationPlan>, StoreError> {
    let path = job_dir.join(PLAN_FILENAME);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(&path)?;
    let plan: IterationPlan = serde_json::from_slice(&bytes)?;
    Ok(Some(plan))
}

/// Atomically write the plan to disk (tmp + rename) so a crash mid-write
/// can't leave a half-written JSON behind.
pub fn save_plan(job_dir: &Path, plan: &IterationPlan) -> Result<PathBuf, StoreError> {
    std::fs::create_dir_all(job_dir)?;
    let final_path = job_dir.join(PLAN_FILENAME);
    let tmp_path = job_dir.join(format!("{PLAN_FILENAME}.tmp"));
    let json = serde_json::to_string_pretty(plan)?;
    std::fs::write(&tmp_path, json)?;
    std::fs::rename(&tmp_path, &final_path)?;
    Ok(final_path)
}

/// Append a history entry and persist. Cheap, called after every stage.
pub fn append_history(
    job_dir: &Path,
    plan: &mut IterationPlan,
    stage: &str,
    kind: HistoryKind,
    message: &str,
) -> Result<(), StoreError> {
    plan.history.push(HistoryEntry {
        at_unix_ms: crate::types::current_time_unix_ms(),
        stage: stage.to_string(),
        kind,
        message: message.to_string(),
    });
    save_plan(job_dir, plan)?;
    Ok(())
}

/// Extract mechanical regions from a slice of annotation JSON values. No
/// interpretation, no LLM — just connected-component-style clustering by
/// bounding-box proximity.
///
/// Input shape (what `canvas.rs` writes today):
/// - Each annotation is a JSON object with a geometric shape. We look for
///   any of `bounds: {x, y, w, h}`, `points: [{x, y}, …]`, `x`+`y`+`w`+`h`
///   at the top level, or a `text` field with `x`+`y`.
/// - A `stroke` / `color` / `strokeColor` field contributes the dominant
///   color.
/// - A `text` field contributes a `labels[]` entry verbatim.
///
/// Unknown / unparseable shapes are skipped silently — this is a
/// best-effort index, never the source of truth (the PNG is).
pub fn extract_regions(annotations: &[serde_json::Value]) -> Vec<CanvasRegion> {
    let mut raw: Vec<RawAnnotation> = Vec::new();
    for ann in annotations {
        if let Some(r) = parse_annotation(ann) {
            raw.push(r);
        }
    }
    cluster(&raw)
}

#[derive(Debug, Clone)]
struct RawAnnotation {
    bbox: [f32; 4],
    color: Option<String>,
    label: Option<String>,
}

fn parse_annotation(v: &serde_json::Value) -> Option<RawAnnotation> {
    let color = v
        .get("color")
        .or_else(|| v.get("strokeColor"))
        .or_else(|| v.get("stroke"))
        .and_then(|s| s.as_str())
        .map(normalize_color);
    let label = v
        .get("text")
        .or_else(|| v.get("label"))
        .and_then(|s| s.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.trim().is_empty());

    // Shape A: explicit bounds/bbox object.
    for key in ["bounds", "bbox"] {
        if let Some(b) = v.get(key) {
            if let Some(bb) = read_xywh(b) {
                return Some(RawAnnotation {
                    bbox: bb,
                    color,
                    label,
                });
            }
        }
    }
    // Shape B: x/y/w/h at the top level.
    if let Some(bb) = read_xywh(v) {
        return Some(RawAnnotation {
            bbox: bb,
            color,
            label,
        });
    }
    // Shape C: polyline / freehand with `points[]`.
    if let Some(points) = v.get("points").and_then(|p| p.as_array()) {
        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;
        let mut count = 0u32;
        for p in points {
            let x = p.get("x").and_then(|n| n.as_f64()).map(|n| n as f32);
            let y = p.get("y").and_then(|n| n.as_f64()).map(|n| n as f32);
            if let (Some(x), Some(y)) = (x, y) {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
                count += 1;
            }
        }
        if count > 0 {
            let bbox = [min_x, min_y, (max_x - min_x).max(1.0), (max_y - min_y).max(1.0)];
            return Some(RawAnnotation {
                bbox,
                color,
                label,
            });
        }
    }
    // Shape D: text note with just x/y.
    if let (Some(x), Some(y)) = (
        v.get("x").and_then(|n| n.as_f64()),
        v.get("y").and_then(|n| n.as_f64()),
    ) {
        return Some(RawAnnotation {
            bbox: [x as f32, y as f32, 1.0, 1.0],
            color,
            label,
        });
    }
    None
}

fn read_xywh(v: &serde_json::Value) -> Option<[f32; 4]> {
    let x = v.get("x").and_then(|n| n.as_f64())?;
    let y = v.get("y").and_then(|n| n.as_f64())?;
    let w = v
        .get("w")
        .or_else(|| v.get("width"))
        .and_then(|n| n.as_f64())
        .unwrap_or(0.0)
        .max(1.0);
    let h = v
        .get("h")
        .or_else(|| v.get("height"))
        .and_then(|n| n.as_f64())
        .unwrap_or(0.0)
        .max(1.0);
    Some([x as f32, y as f32, w as f32, h as f32])
}

fn normalize_color(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    trimmed.to_lowercase()
}

/// Merge overlapping / nearby bboxes into a single region. Iterative
/// greedy pass — good enough for feedback UIs with O(10) strokes.
fn cluster(raws: &[RawAnnotation]) -> Vec<CanvasRegion> {
    if raws.is_empty() {
        return Vec::new();
    }
    // Proximity: expand each bbox by PAD px and consider clusters merged
    // if expanded bboxes overlap. Calibrated for feedback drawings where
    // strokes of the "same circle" are typically within ~40 px of each
    // other.
    const PAD: f32 = 32.0;

    let mut clusters: Vec<Vec<usize>> = Vec::new();
    for (i, r) in raws.iter().enumerate() {
        let mut merged_into: Option<usize> = None;
        for (ci, cluster) in clusters.iter().enumerate() {
            if cluster.iter().any(|&j| overlaps(r.bbox, raws[j].bbox, PAD)) {
                merged_into = Some(ci);
                break;
            }
        }
        match merged_into {
            Some(ci) => clusters[ci].push(i),
            None => clusters.push(vec![i]),
        }
    }

    // Second pass: merge clusters that now touch after new members joined.
    let mut changed = true;
    while changed {
        changed = false;
        'outer: for i in 0..clusters.len() {
            for j in (i + 1)..clusters.len() {
                let bbox_i = cluster_bbox(&clusters[i], raws);
                let bbox_j = cluster_bbox(&clusters[j], raws);
                if overlaps(bbox_i, bbox_j, PAD) {
                    let moved = clusters.remove(j);
                    clusters[i].extend(moved);
                    changed = true;
                    break 'outer;
                }
            }
        }
    }

    clusters
        .into_iter()
        .enumerate()
        .map(|(idx, members)| {
            let bbox = cluster_bbox(&members, raws);
            let dominant_color = dominant_color(&members, raws);
            let mut labels: Vec<String> = members
                .iter()
                .filter_map(|&m| raws[m].label.clone())
                .collect();
            labels.sort();
            labels.dedup();
            CanvasRegion {
                id: format!("R{}", idx + 1),
                bbox,
                stroke_count: members.len() as u32,
                dominant_color,
                labels,
            }
        })
        .collect()
}

fn overlaps(a: [f32; 4], b: [f32; 4], pad: f32) -> bool {
    let ax2 = a[0] + a[2] + pad;
    let ay2 = a[1] + a[3] + pad;
    let bx2 = b[0] + b[2] + pad;
    let by2 = b[1] + b[3] + pad;
    let ax1 = a[0] - pad;
    let ay1 = a[1] - pad;
    let bx1 = b[0] - pad;
    let by1 = b[1] - pad;
    ax1 < bx2 && bx1 < ax2 && ay1 < by2 && by1 < ay2
}

fn cluster_bbox(members: &[usize], raws: &[RawAnnotation]) -> [f32; 4] {
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = f32::MIN;
    let mut max_y = f32::MIN;
    for &i in members {
        let b = raws[i].bbox;
        min_x = min_x.min(b[0]);
        min_y = min_y.min(b[1]);
        max_x = max_x.max(b[0] + b[2]);
        max_y = max_y.max(b[1] + b[3]);
    }
    [min_x, min_y, (max_x - min_x).max(1.0), (max_y - min_y).max(1.0)]
}

fn dominant_color(members: &[usize], raws: &[RawAnnotation]) -> Option<String> {
    use std::collections::HashMap;
    let mut counts: HashMap<String, u32> = HashMap::new();
    for &i in members {
        if let Some(c) = &raws[i].color {
            if !c.is_empty() {
                *counts.entry(c.clone()).or_default() += 1;
            }
        }
    }
    counts.into_iter().max_by_key(|(_, n)| *n).map(|(c, _)| c)
}

/// Collect every region ID that is declared anywhere in the plan's
/// motivated_by_regions fields. Used by the final validator to ensure
/// every canvas region has an owner.
pub fn owned_region_ids(plan: &IterationPlan) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for e in &plan.backend.entities {
        ids.extend(e.motivated_by_regions.iter().cloned());
    }
    for c in &plan.backend.commands {
        ids.extend(c.motivated_by_regions.iter().cloned());
    }
    for r in &plan.frontend.routes {
        ids.extend(r.motivated_by_regions.iter().cloned());
    }
    for c in &plan.frontend.components {
        ids.extend(c.motivated_by_regions.iter().cloned());
    }
    for s in &plan.e2e.scenarios {
        ids.extend(s.motivated_by_regions.iter().cloned());
    }
    ids.sort();
    ids.dedup();
    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    #[test]
    fn plan_round_trips_empty() {
        let plan = IterationPlan::default();
        let s = serde_json::to_string(&plan).unwrap();
        let back: IterationPlan = serde_json::from_str(&s).unwrap();
        assert_eq!(back, plan);
    }

    #[test]
    fn plan_persists_and_reloads() {
        let dir = tempdir().unwrap();
        let mut plan = IterationPlan {
            schema_version: PLAN_SCHEMA_VERSION,
            app: AppIdentity {
                name: "Demo".into(),
                one_liner: "demo".into(),
                job_id: "job-1".into(),
                iteration: 1,
                ..Default::default()
            },
            ..Default::default()
        };
        plan.backend.commands.push(CommandPlan {
            name: "create_thing".into(),
            input: "CreateThingPayload".into(),
            output: "Thing".into(),
            motivated_by_regions: vec!["R1".into()],
            summary: "Create a Thing".into(),
        });

        save_plan(dir.path(), &plan).unwrap();
        let back = load_plan(dir.path()).unwrap().unwrap();
        assert_eq!(back.backend.commands.len(), 1);
        assert_eq!(back.app.name, "Demo");
    }

    #[test]
    fn plan_tolerates_unknown_fields() {
        let raw = r#"{
            "schemaVersion": 1,
            "app": {"name": "Demo"},
            "stage": "seeded",
            "canvas": {"pngPath": "/x", "route": "/"},
            "backend": {},
            "unknownFutureField": 42
        }"#;
        let plan: IterationPlan = serde_json::from_str(raw).unwrap();
        assert_eq!(plan.app.name, "Demo");
        assert_eq!(plan.stage, PlanStage::Seeded);
    }

    #[test]
    fn extract_regions_clusters_nearby_strokes() {
        let ann = vec![
            json!({"x": 10, "y": 10, "w": 20, "h": 20, "color": "#ff0000"}),
            json!({"x": 25, "y": 15, "w": 20, "h": 20, "color": "#ff0000"}),
            json!({"x": 400, "y": 400, "w": 10, "h": 10, "color": "#00ff00"}),
        ];
        let regions = extract_regions(&ann);
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0].id, "R1");
        assert_eq!(regions[0].stroke_count, 2);
        assert_eq!(regions[0].dominant_color.as_deref(), Some("#ff0000"));
    }

    #[test]
    fn extract_regions_handles_polyline_points() {
        let ann = vec![json!({
            "type": "stroke",
            "points": [
                {"x": 100, "y": 100},
                {"x": 120, "y": 120},
                {"x": 140, "y": 110}
            ],
            "color": "#0000ff"
        })];
        let regions = extract_regions(&ann);
        assert_eq!(regions.len(), 1);
        let bbox = regions[0].bbox;
        assert!((bbox[0] - 100.0).abs() < 0.01);
        assert!((bbox[1] - 100.0).abs() < 0.01);
    }

    #[test]
    fn extract_regions_collects_labels_verbatim() {
        let ann = vec![
            json!({"x": 10, "y": 10, "w": 20, "h": 20, "text": "MISSING!"}),
            json!({"x": 15, "y": 12, "w": 20, "h": 20, "text": "FIX THIS"}),
        ];
        let regions = extract_regions(&ann);
        assert_eq!(regions.len(), 1);
        assert!(regions[0].labels.contains(&"MISSING!".to_string()));
        assert!(regions[0].labels.contains(&"FIX THIS".to_string()));
    }

    #[test]
    fn extract_regions_empty_input_yields_empty() {
        let regions = extract_regions(&[]);
        assert!(regions.is_empty());
    }

    #[test]
    fn append_history_persists_entries() {
        let dir = tempdir().unwrap();
        let mut plan = IterationPlan::default();
        save_plan(dir.path(), &plan).unwrap();
        append_history(
            dir.path(),
            &mut plan,
            "backend_plan",
            HistoryKind::Started,
            "hello",
        )
        .unwrap();
        let back = load_plan(dir.path()).unwrap().unwrap();
        assert_eq!(back.history.len(), 1);
        assert_eq!(back.history[0].kind, HistoryKind::Started);
        assert_eq!(back.history[0].message, "hello");
    }

    #[test]
    fn history_entry_tolerates_claude_drift() {
        // The shapes Claude actually produces in the wild (and which broke
        // one job before this regression test): `note` instead of `message`,
        // `kind` missing, `atUnixMs` missing.
        let shapes = [
            // Golden shape.
            r#"{"atUnixMs": 42, "stage": "backend_plan", "kind": "validated", "message": "ok"}"#,
            // `note` alias for `message`.
            r#"{"atUnixMs": 42, "stage": "backend_plan", "kind": "note", "note": "ok"}"#,
            // Missing `kind`.
            r#"{"atUnixMs": 42, "stage": "backend_plan", "message": "ok"}"#,
            // Missing `atUnixMs`.
            r#"{"stage": "backend_plan", "kind": "note", "message": "ok"}"#,
            // All optional fields missing — still round-trips as a Note.
            r#"{"stage": "backend_plan"}"#,
            // Even the stage can be absent without aborting the whole plan.
            r#"{}"#,
        ];
        for (i, raw) in shapes.iter().enumerate() {
            let entry: HistoryEntry = serde_json::from_str(raw)
                .unwrap_or_else(|e| panic!("shape {i} failed to decode: {raw}: {e}"));
            // Just confirm it parsed without panic; we don't care about
            // field-level defaults — the validator is what enforces semantics.
            let _ = entry;
        }
    }

    #[test]
    fn plan_with_drifted_history_still_loads() {
        // Reproduces the job-1776948054961 failure: BackendPlan wrote a
        // history entry with `note` instead of `message` and no `kind`.
        // Before the fix, this aborted the whole plan deserialisation.
        let dir = tempdir().unwrap();
        let raw = r#"{
            "schemaVersion": 1,
            "app": {"name": "Demo"},
            "stage": "backend_planned",
            "canvas": {"route": "/"},
            "history": [
                {"stage": "backend_plan", "atUnixMs": 1, "note": "7 entities, 18 commands planned"}
            ]
        }"#;
        std::fs::write(dir.path().join(PLAN_FILENAME), raw).unwrap();
        let plan = load_plan(dir.path()).unwrap().unwrap();
        assert_eq!(plan.stage, PlanStage::BackendPlanned);
        assert_eq!(plan.history.len(), 1);
        assert_eq!(plan.history[0].message, "7 entities, 18 commands planned");
        assert_eq!(plan.history[0].kind, HistoryKind::Note);
    }

    #[test]
    fn owned_region_ids_aggregates_across_sections() {
        let mut plan = IterationPlan::default();
        plan.backend.entities.push(EntityPlan {
            name: "Project".into(),
            fields: vec![],
            motivated_by_regions: vec!["R1".into(), "R2".into()],
        });
        plan.frontend.components.push(ComponentPlan {
            name: "Form".into(),
            module: "app.rs".into(),
            uses_commands: vec![],
            summary: "".into(),
            motivated_by_regions: vec!["R2".into(), "R3".into()],
        });
        let ids = owned_region_ids(&plan);
        assert_eq!(ids, vec!["R1", "R2", "R3"]);
    }
}
