#!/usr/bin/env bash
# evolvo-delegate — supervisor's gateway to the local-model worker.
#
# The supervisor agent (Claude Code / Codex) calls this script during a
# lineage run when it has a tightly-scoped sub-implementation it would
# rather hand to a local model than spend supervisor tokens on. The script
# forwards the spec to `pi` running against the Ollama provider and
# streams the worker's transcript into `pi.log` next to the supervisor's
# own log so reviewers can audit who wrote what.
#
# Contract (read by both supervisor prompt and humans):
#   evolvo-delegate <spec-file>
#     - <spec-file>: path to a markdown/text file describing the subtask.
#       Must include target file paths, exact signatures, required imports,
#       and one acceptance assertion. Loose specs produce drift.
#
# Environment (set by the runner before spawning the supervisor):
#   EVOLVO_DELEGATE_PROVIDER     — provider slug (default: "ollama")
#   EVOLVO_DELEGATE_MODEL        — model id (default: "gemma4")
#   EVOLVO_DELEGATE_LOG          — absolute path to the per-job pi.log
#                                  (defaults to ./pi.log under the worktree)
#   EVOLVO_DELEGATE_TIMEOUT      — seconds, default 180
#   EVOLVO_DELEGATE_SKIP_CHECK   — set to "1" to skip the post-delegation
#                                  cargo-check quality gate (e.g. for specs
#                                  that only touch markdown / JSON / fixtures)
#   EVOLVO_DELEGATE_EVENTS_LOG   — JSONL event stream path. The runner sets
#                                  this to <job_dir>/pi-events.jsonl so the
#                                  Lineage UI can aggregate counts. One line
#                                  per invocation, written at exit (any rc).
#   EVOLVO_DELEGATE_STAGE_SLUG   — pipeline stage slug (e.g. "backend_impl"
#                                  or "main" for single-session runs). Used
#                                  for the per-stage hard cap and for the
#                                  by-stage breakdown in the UI.
#   EVOLVO_DELEGATE_MAX_PER_STAGE — integer hard cap on delegations per
#                                   stage (default: 5). When the cap is
#                                   reached the wrapper refuses to invoke
#                                   pi without burning supervisor time.
#
# Exit codes:
#   0   — pi run completed AND the cargo-check gate passed
#   2   — usage error (missing/empty spec)
#   3   — pi binary not on PATH
#   4   — pi exited non-zero, OR cargo-check found errors after pi's edits
#         (in either case the supervisor must take the task back)
#   5   — per-stage cap reached; supervisor should finish the rest itself
#   124 — pi timed out (same convention as `timeout(1)`)
#
# This script is intentionally thin. Per-task quality gates (`cargo check`
# after delegation, retry policy, hard caps on delegation count) live in
# the runner / supervisor prompt, not here.

set -euo pipefail

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'
  exit 0
fi

if [[ $# -lt 1 ]]; then
  echo "evolvo-delegate: missing <spec-file> argument" >&2
  echo "usage: evolvo-delegate <spec-file>" >&2
  exit 2
fi

SPEC_FILE="$1"
if [[ ! -s "$SPEC_FILE" ]]; then
  echo "evolvo-delegate: spec file is missing or empty: $SPEC_FILE" >&2
  exit 2
fi

if ! command -v pi >/dev/null 2>&1; then
  echo "evolvo-delegate: 'pi' is not on PATH — install pi-agent and configure ~/.pi/agent/models.json" >&2
  exit 3
fi

PROVIDER="${EVOLVO_DELEGATE_PROVIDER:-ollama}"
MODEL="${EVOLVO_DELEGATE_MODEL:-gemma4}"
LOG_FILE="${EVOLVO_DELEGATE_LOG:-./pi.log}"
TIMEOUT_S="${EVOLVO_DELEGATE_TIMEOUT:-180}"
EVENTS_LOG="${EVOLVO_DELEGATE_EVENTS_LOG:-}"
STAGE_SLUG="${EVOLVO_DELEGATE_STAGE_SLUG:-main}"
MAX_PER_STAGE="${EVOLVO_DELEGATE_MAX_PER_STAGE:-5}"

mkdir -p "$(dirname -- "$LOG_FILE")" 2>/dev/null || true
[[ -n "$EVENTS_LOG" ]] && mkdir -p "$(dirname -- "$EVENTS_LOG")" 2>/dev/null || true

# now_ms — wall-clock unix milliseconds, portable across GNU and BSD.
# `date +%s%3N` is GNU-only; on macOS BSD `date` it silently emits a
# literal `N` after the seconds, breaking arithmetic. We probe for
# `gdate` (Homebrew coreutils), then python3, then perl, then fall back
# to second-resolution multiplied by 1000.
now_ms() {
  if command -v gdate >/dev/null 2>&1; then
    gdate +%s%3N
  elif command -v python3 >/dev/null 2>&1; then
    python3 -c 'import time; print(int(time.time()*1000))'
  elif command -v perl >/dev/null 2>&1; then
    perl -MTime::HiRes=time -e 'printf "%d\n", time*1000'
  else
    echo $(( $(date +%s) * 1000 ))
  fi
}

# Per-stage hard cap. Counts prior `result` events for this stage in the
# JSONL log; refuses if at or above the cap. Skipped when no events log
# is configured (single dev runs of the wrapper) or `jq` is missing —
# the cap is a soft safety net, not a security boundary, so degrading
# to "no enforcement" is acceptable when the tooling isn't present.
STAGE_COUNT=0
if [[ -n "$EVENTS_LOG" && -f "$EVENTS_LOG" ]] && command -v jq >/dev/null 2>&1; then
  STAGE_COUNT=$(jq -s --arg s "$STAGE_SLUG" \
    '[.[] | select(.kind == "result" and .stage == $s)] | length' \
    "$EVENTS_LOG" 2>/dev/null || echo 0)
fi
if [[ "$STAGE_COUNT" =~ ^[0-9]+$ ]] && (( STAGE_COUNT >= MAX_PER_STAGE )); then
  msg="evolvo-delegate: per-stage delegation cap reached ($STAGE_COUNT/$MAX_PER_STAGE for stage '$STAGE_SLUG') — finish remaining work yourself"
  echo "$msg" >&2
  if [[ -n "$EVENTS_LOG" ]]; then
    cap_ts="$(now_ms)"
    printf '{"kind":"capped","ts_unix_ms":%s,"stage":"%s","spec_file":"%s","cap":%s}\n' \
      "$cap_ts" "$STAGE_SLUG" "${SPEC_FILE//\"/\\\"}" "$MAX_PER_STAGE" >> "$EVENTS_LOG"
  fi
  exit 5
fi

# Track wall-clock for the events record (millisecond resolution; see
# `now_ms` above for the cross-platform implementation).
START_MS="$(now_ms)"
START_TS="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
{
  echo "===== evolvo-delegate $START_TS ====="
  echo "spec_file: $SPEC_FILE"
  echo "provider:  $PROVIDER"
  echo "model:     $MODEL"
  echo "timeout_s: $TIMEOUT_S"
  echo "----- spec -----"
  cat -- "$SPEC_FILE"
  echo "----- pi output -----"
} >> "$LOG_FILE"

# Pi reads the spec from a file via @-syntax; -p makes it non-interactive.
# --no-session keeps the supervisor's session graph clean — each delegation
# is a standalone turn. We allowlist read/write/edit/bash so pi can land
# the implementation directly into the worktree.
RC=0
if command -v gtimeout >/dev/null 2>&1; then
  TIMEOUT_BIN=gtimeout
elif command -v timeout >/dev/null 2>&1; then
  TIMEOUT_BIN=timeout
else
  TIMEOUT_BIN=""
fi

PI_INVOKE=(pi -p
            --provider "$PROVIDER"
            --model "$MODEL"
            --no-session
            --tools read,write,edit,bash
            "@$SPEC_FILE")

if [[ -n "$TIMEOUT_BIN" ]]; then
  "$TIMEOUT_BIN" "$TIMEOUT_S" "${PI_INVOKE[@]}" 2>&1 | tee -a "$LOG_FILE"
  RC=${PIPESTATUS[0]}
else
  "${PI_INVOKE[@]}" 2>&1 | tee -a "$LOG_FILE"
  RC=${PIPESTATUS[0]}
fi

END_TS="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
END_MS="$(now_ms)"
RUNTIME_MS=$(( END_MS - START_MS ))
echo "----- end ($END_TS, rc=$RC, runtime_ms=$RUNTIME_MS) -----" >> "$LOG_FILE"

# emit_event <final_rc> <gate_status>
# Appends one JSONL line to $EVENTS_LOG. Schema is forward-compatible —
# the host parser uses #[serde(default)] on collection fields, so
# adding a key here doesn't break older builds.
emit_event() {
  local final_rc="$1"
  local gate="$2"
  [[ -z "$EVENTS_LOG" ]] && return 0
  local spec_basename
  spec_basename="$(basename -- "$SPEC_FILE")"
  printf '{"kind":"result","ts_unix_ms":%s,"stage":"%s","spec_file":"%s","provider":"%s","model":"%s","rc":%s,"runtime_ms":%s,"gate":"%s"}\n' \
    "$START_MS" \
    "$STAGE_SLUG" \
    "${spec_basename//\"/\\\"}" \
    "$PROVIDER" \
    "$MODEL" \
    "$final_rc" \
    "$RUNTIME_MS" \
    "$gate" \
    >> "$EVENTS_LOG"
}

if [[ $RC -eq 124 ]]; then
  echo "evolvo-delegate: pi timed out after ${TIMEOUT_S}s" >&2
  emit_event 124 "skipped_timeout"
  exit 124
fi
if [[ $RC -ne 0 ]]; then
  echo "evolvo-delegate: pi exited with rc=$RC — supervisor should take the task back" >&2
  emit_event 4 "skipped_pi_failed"
  exit 4
fi

# --- Post-delegation quality gate ------------------------------------------
# Pi may have produced plausible-looking but compiler-rejected Rust. Before
# we let the supervisor consider the task done, run `cargo check` so a
# broken delegation is caught here, not three stages later. The gate is
# best-effort: skipped when the spec only touched non-code files (no `.rs`
# changes in `git status`), when the user opted out via env, or when this
# isn't a Cargo workspace.

if [[ "${EVOLVO_DELEGATE_SKIP_CHECK:-0}" == "1" ]]; then
  echo "evolvo-delegate: cargo-check gate skipped (EVOLVO_DELEGATE_SKIP_CHECK=1)" >> "$LOG_FILE"
  emit_event 0 "skipped_opt_out"
  exit 0
fi

if [[ ! -f Cargo.toml ]]; then
  # Not a Rust workspace — nothing to gate. Iterations that rewrote the
  # stack (e.g. a Next.js NewApp) hit this path and skip the gate
  # cleanly. The supervisor remains responsible for verifying the build
  # in its own way per `prompts/implementation.md` step 6.
  echo "evolvo-delegate: cargo-check gate skipped (no Cargo.toml in cwd)" >> "$LOG_FILE"
  emit_event 0 "skipped_no_cargo"
  exit 0
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "evolvo-delegate: cargo-check gate skipped (cargo not on PATH)" >> "$LOG_FILE"
  emit_event 0 "skipped_no_cargo"
  exit 0
fi

# Only gate when pi actually touched Rust source. Specs that target
# styles.css / fixtures / markdown shouldn't pay the 10–60s `cargo check`
# tax. We use `git diff --name-only HEAD` so unstaged edits land in the
# count even though the supervisor hasn't committed yet.
RS_CHANGED="$(git diff --name-only HEAD -- '*.rs' 2>/dev/null | head -1 || true)"
if [[ -z "$RS_CHANGED" ]]; then
  echo "evolvo-delegate: cargo-check gate skipped (no .rs changes since HEAD)" >> "$LOG_FILE"
  emit_event 0 "skipped_no_rs"
  exit 0
fi

CHECK_TS="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo "----- cargo check --workspace ($CHECK_TS) -----" >> "$LOG_FILE"
# Capture compiler output to the log AND emit it on stderr so the
# supervisor's tool-call result includes the diagnostic. We keep the
# default cargo cache (no --frozen) so subsequent gates are fast.
if ! cargo check --workspace --message-format=short 2>&1 | tee -a "$LOG_FILE" >&2; then
  CHECK_RC=${PIPESTATUS[0]}
  echo "evolvo-delegate: cargo check failed (rc=$CHECK_RC) after pi edits — supervisor should revert and retry or take the task back" >&2
  echo "----- gate failed (rc=$CHECK_RC) -----" >> "$LOG_FILE"
  emit_event 4 "failed_cargo_check"
  exit 4
fi
echo "----- gate passed -----" >> "$LOG_FILE"

emit_event 0 "passed"
exit 0
