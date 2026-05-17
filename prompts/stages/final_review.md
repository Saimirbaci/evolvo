# Stage 7 / 7 — Final review

{{CONTRACT}}

## Preconditions (fail fast)

1. `{{PLAN_PATH}}` parses and all six prior stages have a passing
   history entry. Otherwise: `STAGE_RESULT: fail: prior stage incomplete`.
2. The dev server is running on port `{{PORT}}`
   (`lsof -iTCP:{{PORT}} -sTCP:LISTEN` returns a single PID). Otherwise:
   `STAGE_RESULT: fail: dev server not listening on port {{PORT}}`.

The six prior stages have each been validated. This stage has two
jobs: (a) confirm the build is shippable, (b) **rewrite the repo's
context files** so the next iteration's agent sees the new app, not
the host Evolvo shell that came before it.

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
4. The dev-server is running on port `{{PORT}}`.

## Rewrite the project guide for the new app (load-bearing)

This step is the single biggest determinant of whether **the next
iteration** (the user's first bug fix or feature request on this
NewApp) succeeds. The agent on iteration 2 reads these files FIRST.
If they still describe the host Evolvo's Tauri/Leptos shell — not the
app you just built — iteration 2 will make a wrong fix against
imagined architecture.

Read the plan's `app.name`, `app.oneLiner`, `app.domain`,
`backend.entities`, `backend.commands`, `frontend.routes`, and
`frontend.components`. Then rewrite each of the files below so they
describe **this app**, not the host shell. Do not just append — replace
the stale sections wholesale. Stale docs are worse than no docs.

### `CLAUDE.md` (worktree root)

Replace it with a fresh guide that opens by naming the app, its
domain, and its purpose. Required sections (in this order):

1. **What this app is** — one paragraph naming the app (from
   `plan.app.name`), the domain, and what a user does with it.
2. **Stack snapshot** — table of layers (UI / host / build / storage)
   with entry-point file paths. Update if the iteration changed the
   stack.
3. **Commands you'll actually run** — `cargo check`, `cargo test`,
   `cargo tauri dev`, `trunk serve`, plus any new ones the iteration
   added. The Run command for the iteration's port.
4. **Core domain model** — list each entity from
   `plan.backend.entities` with its fields. Where they're defined.
5. **Tauri commands** — list each one from `plan.backend.commands`
   with its purpose. Where to find the body. Reminder about the
   three-step registration (commands.rs → invoke_handler → interop.rs).
6. **Frontend structure** — list each route + component from
   `plan.frontend`. Where they live.
7. **Conventions** — keep / adapt the conventions section from the
   host Evolvo, but trim anything that no longer applies (e.g. if
   you removed the canvas overlay, drop its rules).
8. **Product invariants** — keep these intact, they apply to every
   Evolvo-derived app:
   - Lineage pipeline always reachable.
   - Feedback Overlay always reachable.
   - Canvas overlay on every page (not its own tab).
   - One FAB opens both Canvas and Feedback together.
   - Lineagees are saveable + forkable into standalone apps.

Forbidden in the rewrite:
- The phrase `Evolvo — Repository Guide for Claude` (host's exact
  header — its presence is the fingerprint that says you didn't
  rewrite).
- Sentences that begin "Evolvo is a Tauri 2 desktop app" describing
  *this app* as the host shell. (If the iteration genuinely kept
  the Tauri/Leptos stack, rephrase the description to lead with what
  the *user's* app does, not "Evolvo".)
- Stale references to features the iteration removed.

### `.claude/rules/`

Walk the existing files. For each:
- Keep it if the rule still applies to this app (e.g. Rust conventions
  always apply on the default stack).
- Update it if the convention shifted (e.g. new layer naming).
- Delete the file if its subject layer no longer exists (e.g. if the
  iteration removed Tauri, the `rules/tauri/` files are obsolete).

### `.claude/agents/*`

Walk the existing specialist agents. For each:
- Update the `description:` frontmatter so the agent's domain matches
  the new app. `staff-feedback`, `staff-build-engineer`, and
  `staff-architect-self-evolving-software` are all still relevant —
  but their bodies probably reference Evolvo specifics that should
  now reference the new app.
- Add a new specialist if the app introduces a new domain that
  deserves one (e.g. if you built a markdown editor, a
  `staff-editor-ux` agent makes sense).
- Delete an agent only if it's clearly inapplicable (rare — most of
  the host's agents generalise).

### `.claude/skills/*`

Same treatment: update bodies, prune what no longer applies, add what
the new app needs.

### Concrete success bar

After your rewrite, a fresh reader of `CLAUDE.md` who has never seen
this repo should be able to answer:
- "What does this app do?"
- "Which files own the domain model?"
- "How do I run it?"
- "What are the commands the UI calls into?"

…without reading anything else. If they'd answer "this is a meta-app
that uses canvas feedback to evolve itself", you didn't rewrite.

## What to write into `plan.json`

- Update `plan.stage` to `"completed"` and append a final `history`
  entry `{kind: "note", stage: "final_review", message: "ready — port
  {{PORT}}; CLAUDE.md rewritten for <app.name>"}`.
- Do not rewrite earlier sections of `plan.json`.

## When finished

Reply per the Operating contract. Include `port: {{PORT}}` in your
summary AND a one-line confirmation that you rewrote `CLAUDE.md`,
`.claude/agents/*`, and `.claude/skills/*` for the new app. The final
line must be `STAGE_RESULT: ok` (or `STAGE_RESULT: fail: <reason>` if
any of the four invariants regressed, any check failed, OR you could
not honestly rewrite the context files — the runner will roll the
build back rather than ship it).
