---
name: evolvo-workspace
description: Inspect, synthesize, or reset the local Evolvo workspace at ~/.evolvo/evolvo_workspace/ (or $EVOLVO_WORKSPACE_ROOT). Use when the user asks to "seed test feedback", "show what's in the workspace", "reset the workspace", "create a fixture workspace", or needs to reason about the on-disk state of feedback/lineage jobs/attachments.
---

# Evolvo Workspace Skill

The Evolvo workspace is a directory of JSON files. This skill is the map.

## Product invariants (always hold)

Authoritative text: `.claude/rules/common/product-invariants.md`.

- **I-P1.** Lineage pipeline is permanent — never delete `lineage_jobs/` wholesale, never "flatten" it into `feedback/`.
- **I-P2.** Feedback Overlay is always reachable — workspace shape must never encode a "feedback disabled" state.
- **I-P3.** The drawing board is always reachable — canvas attachments (`attachments/{id}/canvas.png`) are one concrete artifact of this, but the product guarantee is the affordance, not the file.
- **I-P4.** lineagees are saveable and forkable into another app. A workspace — or a subset of it scoped to one lineage job plus its feedback and attachments — must be exportable as a self-contained bundle that can seed a new Evolvo app under a fresh `EVOLVO_WORKSPACE_ROOT`. Never synthesize workspace data with host-absolute paths or machine-specific identifiers that would block this.

When seeding fixtures or proposing layout changes, verify all four hold.

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

Minimum valid `lineageJobRecord`:

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
- **Never edit feedback JSON to rewrite what a user wrote** — status/updatedAtUnixMs/lineageJobId only.
- **Never introduce new filenames** in `attachments/` that wouldn't pass `sanitise_filename` (keep to `[A-Za-z0-9._-]`).
- **Never add a new top-level directory** under the workspace without updating `WorkspaceLayout::directories()` in `store.rs` — the `init_workspace` call creates exactly that set.
- **Never hardcode `~/.evolvo`** outside `store::default_workspace_root()`.

## When to escalate

- Schema changes → `staff-architect-self-evolving-software`
- Attachment-handling changes → `staff-architect-self-evolving-software` (security boundary)
- `jq`-based bulk edits to a workspace → refuse; route through Tauri commands so the user can replay.
