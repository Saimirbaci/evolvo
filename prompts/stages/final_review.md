# Stage 7 / 7 — Final review

The six prior stages have each been validated. This stage is read-only
housekeeping: confirm the iteration is shippable and leave breadcrumbs so
the reviewer lands on a live build.

## Your inputs

`{{PLAN_PATH}}` and `{{WORKTREE}}`.

## What to check

1. Every region id in `plan.canvas.regions` appears in at least one
   `motivatedByRegions` list across the plan. If any is orphaned,
   append a `history` entry `{kind: "note", message: "orphan region
   <id> — claimed by nothing"}` and stop with `final_review failed`.
2. The four product invariants still hold in the running UI:
   - Lineage page is reachable from every route.
   - The Feedback FAB is present on every route.
   - Canvas overlay opens on top of the current route (not as its own
     tab), driven by the same `panel_open` signal as the feedback panel.
   - Lineage records are saveable (no change needed — Evolvo enforces
     this by storage shape, just confirm no migration broke it).
3. `cargo check --workspace` and `cargo test -p evolvo_desktop --lib`
   pass on the worktree.
4. The iteration's dev-server is running on port `1530 + {{ITERATION}}`.

## What to write

- Update `plan.stage` to `"completed"` and append a final `history`
  entry `{kind: "note", stage: "final_review", message: "ready — port
  <N>"}`.
- Do not rewrite earlier sections.

## When finished

Reply with ONLY:

```
final_review done
port: <N>
```
