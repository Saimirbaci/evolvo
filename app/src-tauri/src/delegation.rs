//! Aggregator for the per-job pi-events.jsonl stream.
//!
//! The wrapper script (`scripts/evolvo-delegate.sh`) appends one JSON
//! line per delegation invocation. This module reads those lines and
//! produces a [`DelegationSummary`] for the Lineage UI. Parsing is
//! intentionally lenient: a malformed line is skipped rather than
//! poisoning the whole summary, because the summary surface is
//! informational — it must never block the user from interacting with
//! the lineage job.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::Deserialize;

use crate::types::{DelegationStageStat, DelegationSummary};

/// One JSONL line from the wrapper. The wrapper writes `kind: "result"`
/// for completed invocations and `kind: "capped"` when it refuses to
/// invoke pi because the per-stage cap was reached. Unknown kinds are
/// ignored so the schema can grow without breaking older readers.
#[derive(Debug, Deserialize)]
struct Event {
    kind: String,
    #[serde(default)]
    stage: String,
    #[serde(default)]
    rc: i32,
    #[serde(default)]
    runtime_ms: u64,
}

/// Read `<job_dir>/pi-events.jsonl` if it exists and aggregate it into
/// a [`DelegationSummary`]. Returns `Ok(None)` when the file is absent
/// — that's the steady state for jobs the user didn't opt into
/// delegation on, and the UI treats it as "no local-worker section".
pub fn read_summary(job_dir: &Path) -> std::io::Result<Option<DelegationSummary>> {
    let path = job_dir.join(crate::runner::PI_EVENTS_FILENAME);
    if !path.is_file() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path)?;
    Ok(Some(aggregate(&raw)))
}

/// Pure function over the JSONL bytes. Split out so unit tests don't
/// have to go through the filesystem. Tolerates blank lines and lines
/// that fail to deserialise.
pub fn aggregate(raw: &str) -> DelegationSummary {
    let mut total = 0u32;
    let mut failed = 0u32;
    let mut capped = 0u32;
    let mut runtime_ms = 0u64;
    // BTreeMap so the by-stage list comes out in a stable order — the
    // UI renders this verbatim, and a fluctuating order would be
    // distracting on every reload.
    let mut by_stage: BTreeMap<String, DelegationStageStat> = BTreeMap::new();

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(ev) = serde_json::from_str::<Event>(line) else {
            continue;
        };
        let stage_key = if ev.stage.is_empty() {
            "main".to_string()
        } else {
            ev.stage.clone()
        };
        let stat = by_stage
            .entry(stage_key.clone())
            .or_insert_with(|| DelegationStageStat {
                stage: stage_key,
                ..Default::default()
            });
        match ev.kind.as_str() {
            "result" => {
                total += 1;
                stat.total += 1;
                stat.runtime_ms += ev.runtime_ms;
                runtime_ms += ev.runtime_ms;
                if ev.rc != 0 {
                    failed += 1;
                    stat.failed += 1;
                }
            }
            "capped" => {
                capped += 1;
                stat.capped += 1;
            }
            _ => {}
        }
    }

    DelegationSummary {
        total,
        failed,
        capped,
        runtime_ms,
        by_stage: by_stage.into_values().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_yields_zero_summary() {
        let s = aggregate("");
        assert_eq!(s, DelegationSummary::default());
    }

    #[test]
    fn aggregates_results_and_failures_per_stage() {
        let raw = r#"{"kind":"result","stage":"backend_impl","rc":0,"runtime_ms":1200}
{"kind":"result","stage":"backend_impl","rc":4,"runtime_ms":300}
{"kind":"result","stage":"frontend_impl","rc":0,"runtime_ms":800}
{"kind":"capped","stage":"backend_impl"}
"#;
        let s = aggregate(raw);
        assert_eq!(s.total, 3);
        assert_eq!(s.failed, 1);
        assert_eq!(s.capped, 1);
        assert_eq!(s.runtime_ms, 2300);
        assert_eq!(s.by_stage.len(), 2);

        let backend = s
            .by_stage
            .iter()
            .find(|x| x.stage == "backend_impl")
            .expect("backend stage present");
        assert_eq!(backend.total, 2);
        assert_eq!(backend.failed, 1);
        assert_eq!(backend.capped, 1);
        assert_eq!(backend.runtime_ms, 1500);

        let frontend = s
            .by_stage
            .iter()
            .find(|x| x.stage == "frontend_impl")
            .expect("frontend stage present");
        assert_eq!(frontend.total, 1);
        assert_eq!(frontend.failed, 0);
        assert_eq!(frontend.runtime_ms, 800);
    }

    #[test]
    fn malformed_lines_are_skipped() {
        let raw = r#"{"kind":"result","stage":"main","rc":0,"runtime_ms":100}
not json at all
{"kind":"weird_event_we_dont_know"}
{"kind":"result","stage":"main","rc":0,"runtime_ms":50}
"#;
        let s = aggregate(raw);
        assert_eq!(s.total, 2);
        assert_eq!(s.runtime_ms, 150);
        assert_eq!(s.by_stage.len(), 1);
    }

    #[test]
    fn missing_stage_field_falls_back_to_main() {
        let raw = r#"{"kind":"result","rc":0,"runtime_ms":42}
"#;
        let s = aggregate(raw);
        assert_eq!(s.total, 1);
        assert_eq!(s.by_stage.len(), 1);
        assert_eq!(s.by_stage[0].stage, "main");
    }
}
