---
name: evolvo
description: Map of the Evolvo project — what Evolvo is, how the lineage pipeline works, the Canvas + Feedback overlay model, the multi-agent backends, and how to inspect/seed/reset the local workspace at ~/.evolvo/evolvo_workspace/ (or $EVOLVO_WORKSPACE_ROOT). Use when the user asks "what is Evolvo?", needs to reason about the product, or asks to "seed test feedback", "show what's in the workspace", "reset the workspace", or "create a fixture workspace".
---

# Evolvo Skill

This skill is the map of the Evolvo project — both the product (what it is, how it works) and its on-disk state (the workspace). For deeper detail, the canonical guide is `CLAUDE.md` at the repo root; product invariants live in `.claude/rules/common/product-invariants.md`. Treat this file as a quick orientation, not a substitute.

## What Evolvo is

Evolvo is a local-first **self-evolving desktop app**. The user runs the app, draws / pastes / records feedback on a per-page Canvas overlay, and submits it. Each submission becomes a **lineage job** that a coding-agent backend (Claude Code, Codex, Gemini, or Forge) picks up to produce the next iteration of the app itself. The user reviews, approves, and promotes iterations into a NewApp they can run, fork, save, and rename. Feedback → iteration → promoted app, all on disk, no server, no cloud.

## Stack at a glance

| Layer        | Tech                                         | Entry                       |
|--------------|----------------------------------------------|-----------------------------|
| Desktop host | Tauri 2 (Rust)                               | `app/src-tauri/src/main.rs` |
| Frontend     | Leptos 0.8 CSR + wasm-bindgen + web-sys      | `app/ui/src/main.rs`        |
| UI build     | Trunk → `app/ui/dist/`                       | `app/ui/Trunk.toml`         |
| Storage      | JSON files under `~/.evolvo/evolvo_workspace/` | `app/src-tauri/src/store.rs` |

`EVOLVO_WORKSPACE_ROOT` overrides the default workspace root.

## The lineage pipeline (core concept)

Submitting feedback automatically enqueues a `LineageJobRecord`. Status machine in `app/src-tauri/src/lineage.rs`:

```
pending → triaging → planned → implementing → build_ready → merging → promoted
                                                                  ↘ rejected | failed
```

`build_ready` is the human-approval gate (`can_approve()` gates UI action). Approved jobs proceed to `merging` and ultimately `promoted` — at which point the iteration becomes the new NewApp content.

Each iteration runs in its own worktree on dev port `1530 + N`. The job persists which agent ran it (`agent` on `LineageJobRecord` and `run.json`), so retries reuse the same backend.

## Canvas + Feedback overlay model

The single user-visible surface for feedback. Driven by **one** FAB (in `app/ui/src/shell.rs`) bound to a single `panel_open` signal — one click opens the Canvas overlay *and* the Feedback submission panel together.

- The Canvas is a **per-page overlay**, never its own tab. It floats over whatever route the user is currently on, so feedback is always anchored to the actual screen.
- The overlay is **transparent** so the underlying page stays visible. At submit time, `feedback_panel.rs` hides chrome, calls `capture_window_png` (xcap-backed), and composites the page screenshot with the annotation PNG.
- Z-index contract: overlay 50 < panel 55 < FAB stack.

## Shell vs NewApp

Two-layer UI split (load-bearing):

- **`app/ui/src/shell.rs`** — invariant chrome. Owns the app bar, Lineage page, Star Us link, the single Feedback FAB, and the Canvas + feedback panel composition. Exposes `PanelOpen` via `provide_context`. Never re-implemented in `app.rs`.
- **`app/ui/src/app.rs`** — the **NewApp content area**. This is the file iteration authors rewrite. It always mounts `<Shell>` so every NewApp page automatically gets the FAB, the Canvas overlay, and the Lineage page.

## Product invariants (always hold)

Authoritative text: `.claude/rules/common/product-invariants.md`.

- **I-P1.** Lineage pipeline is permanent — never delete `lineage_jobs/` wholesale, never "flatten" it into `feedback/`, never feature-flag it off.
- **I-P2.** Feedback Overlay is always reachable — every screen, every mode. Workspace shape must never encode a "feedback disabled" state.
- **I-P3.** The Canvas is a per-page overlay, not a tab. Canvas attachments (`attachments/{id}/canvas.png`) are one concrete artifact; the product guarantee is the affordance on every route.
- **I-P3b.** One trigger opens **both** Canvas overlay and Feedback panel, driven by a single signal. No second button. Aria-labelled and titled.
- **I-P4.** Lineagees are saveable and forkable into another app. A workspace — or a subset scoped to one lineage job plus its feedback and attachments — must be exportable as a self-contained bundle that can seed a new Evolvo app under a fresh `EVOLVO_WORKSPACE_ROOT`. Never synthesize workspace data with host-absolute paths or machine-specific identifiers.

When seeding fixtures or proposing layout changes, verify all four hold.

## Multi-agent backends

Lineage jobs run on one of four CLIs. The runner symlinks `CLAUDE.md` into per-agent guide names so they all pick up the same project context:

| Backend     | Binary   | Transcript       | Format                         | Guide file       |
|-------------|----------|------------------|--------------------------------|------------------|
| Claude Code | `claude` | `claude.log`     | stream-json JSONL              | `CLAUDE.md`      |
| Codex CLI   | `codex`  | `codex.log`      | `codex exec --json` JSONL      | `AGENTS.md`      |
| Gemini CLI  | `gemini` | `gemini.log`     | free-form text                 | `GEMINI.md`      |
| Forge       | `forge`  | `forge.log`      | provider-dependent             | `AGENTS.md`      |

The multi-stage NewApp pipeline drives off `plan.json` (`app/src-tauri/src/stages.rs`), not the agent log, so it works for every backend. The Feedback panel's Agent chip greys out backends whose binary is not on PATH.

For debugging a Claude run, see the `jq` recipes in `CLAUDE.md` — `claude.log` is JSONL keyed by `type` (`assistant`, `user`, `result`).

---

# Workspace operations

The Evolvo workspace is a directory of JSON files. Everything below is the on-disk view.

## Where it is

```
${EVOLVO_WORKSPACE_ROOT:-$HOME/.evolvo/evolvo_workspace}/
├── feedback/            {id}.json
├── lineage_jobs/        {id}.json
└── attachments/{feedback_id}/
    ├── canvas.png
    ├── paste-N.png
    └── voice.{webm|ogg|m4a|wav}
```

IDs: `feedback` → `fb-<unix_ms>`. Lineage jobs → whatever `lineage.rs` generates (check there, don't assume).

Lineage worktrees (per-iteration build state) live alongside under `lineage_workspaces/<job-id>/` — that's where `claude.log` / `codex.log` / `run.json` are written.

## Schemas

All JSON uses **camelCase** keys. Authoritative definitions: `app/src-tauri/src/types.rs`.

Minimum valid `FeedbackRecord`:

```json
{
  "id": "fb-1000",
  "feedbackType": "bug",
  "status": "new",
  "pageRoute": "/",
  "feedbackText": "example",
  "windowWidth": 1024,
  "windowHeight": 768,
  "createdAtUnixMs": 1000,
  "updatedAtUnixMs": 1000
}
```

Missing optional fields (`annotations`, `pastedImages`, `screenshotFilename`, `voiceFilename`, `voiceTranscript`, `lineageJobId`) are tolerated on decode — see the `tolerates_extra_fields` test.

Minimum valid `LineageJobRecord`:

```json
{
  "id": "job-1",
  "feedbackId": "fb-1000",
  "title": "Fix",
  "summary": "Summary",
  "status": "pending",
  "notes": [],
  "createdAtUnixMs": 0,
  "updatedAtUnixMs": 0
}
```

Enums (snake_case wire values):
- `feedbackType`: `bug | feature_request | improvement | confusion | compliment`
- `status` (feedback): `new | triaged | in_lineage | resolved | rejected`
- `status` (lineage job): `pending | triaging | planned | implementing | build_ready | merging | promoted | rejected | failed`

## Common operations

### Inspect

```bash
WS="${EVOLVO_WORKSPACE_ROOT:-$HOME/.evolvo/evolvo_workspace}"
ls "$WS/feedback" 2>/dev/null | wc -l
for f in "$WS/feedback"/*.json; do
  jq -r '[.id, .status, .feedbackType, .pageRoute] | @tsv' "$f"
done
for f in "$WS/lineage_jobs"/*.json; do
  jq -r '[.id, .iteration, .status, .agent, .title] | @tsv' "$f"
done | sort -k2,2n
```

### Seed a fixture workspace

Use a throwaway path via `EVOLVO_WORKSPACE_ROOT`, never the real `~/.evolvo/evolvo_workspace`:

```bash
export EVOLVO_WORKSPACE_ROOT="$(mktemp -d)/evolvo"
mkdir -p "$EVOLVO_WORKSPACE_ROOT/feedback" "$EVOLVO_WORKSPACE_ROOT/lineage_jobs"
# write a minimum-valid FeedbackRecord as above
```

### Reset

Destructive — always confirm with the user first. Prefer a fresh `$EVOLVO_WORKSPACE_ROOT` over `rm -rf`.

## Rules

- **Never `rm -rf`** a workspace without explicit user confirmation in the current turn.
- **Never edit feedback JSON to rewrite what a user wrote** — status / `updatedAtUnixMs` / `lineageJobId` only.
- **Never introduce new filenames** in `attachments/` that wouldn't pass `sanitise_filename` (keep to `[A-Za-z0-9._-]`).
- **Never add a new top-level directory** under the workspace without updating `WorkspaceLayout::directories()` in `store.rs` — `init_workspace` creates exactly that set.
- **Never hardcode `~/.evolvo`** outside `store::default_workspace_root()`.
- **Never bake host-absolute paths** into seeded records — it breaks I-P4 portability.

## When to escalate

- Schema changes → `staff-architect-self-evolving-software`
- Attachment-handling changes → `staff-architect-self-evolving-software` (security boundary)
- `jq`-based bulk edits to a live workspace → refuse; route through Tauri commands so the user can replay.
- Anything that would weaken I-P1..I-P4 → `staff-product-manager` for the product call, then architect for implementation.
