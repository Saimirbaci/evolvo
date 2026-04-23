# Iteration {n} — {phase}

This Evolvo instance is a self-evolving meta-app. Each approved lineage job is one iteration in the life of the app the user is building on top of the Evolvo shell.

**Latitude for this iteration:** {latitude}

## Invariants you MUST preserve on every iteration, no matter what

Whatever the app becomes, the shell must keep these four surfaces reachable and functional:

1. **Feedback Overlay** — reachable from every screen, every mode. The user must always be able to open the feedback panel and submit new feedback about the page they are on.
2. **Canvas overlay on every page** — the Canvas is NOT a standalone tab or dedicated route. It is an overlay the user can open on top of *any* page of the app to draw / annotate / sketch feedback about *that specific page*. Every route must support opening the Canvas on top of it; the feedback submission records which route the drawing was made on. A design that only lets the user draw on a single "Canvas tab" is wrong — the whole point is per-page visual feedback.
3. **Inbox** — the list/overview of submitted feedback must remain visible and navigable, and each entry must preserve the page/route it was submitted from.
4. **Lineage pipeline** — the feedback → lineage-job state machine (and the Advance / Retry / Reject / Run affordances) must keep working end-to-end so the *next* iteration can happen.

If your change would break any of these four surfaces in the resulting app, it is wrong — redesign the change to preserve them. These invariants are load-bearing; they are what makes iteration N+1 possible.

## ONE button opens BOTH the Canvas overlay AND the Feedback panel — always

This is a hard rule, not a suggestion. The host iteration zero ships a **single Feedback FAB** (`FeedbackFab` in `app/ui/src/shell.rs`) bound to a **single `panel_open: RwSignal<bool>`** signal owned by the invariant shell. Clicking it toggles the feedback surface open; while open, both the drawing surface (Canvas + Toolbar) and the Feedback submission panel are visible and usable together. There is never one button for "draw" and a second button for "send feedback" — they are the same action from the user's point of view.

## Where the NewApp goes — `app.rs` is yours, `shell.rs` is not

The Leptos UI is split into two layers:

- **`app/ui/src/shell.rs`** is the permanent Evolvo chrome: the app bar with the Lineage navigation + "Star Us" link, the Lineage review page, the single Feedback FAB, and the Canvas overlay + feedback panel composition. The shell is what guarantees the four invariants above — the FAB and overlay wrap whatever content renders inside. **`shell.rs` is invariant.** Do not delete, duplicate, or re-implement any of its pieces inside the NewApp. If the chrome genuinely needs to change, edit `shell.rs` directly and keep all four surfaces working.
- **`app/ui/src/app.rs`** is the **NewApp content area**. When the user asks for a new app, this is where you build it: replace `HomePage` with the new app's root component (router, layout, pages, state) and add further modules alongside it. Keep `App` mounting `<Shell>` with the new content as its children. Because the shell wraps the content, every page/route of the NewApp is automatically annotatable — the Canvas overlay mounts on top of whatever `app.rs` renders when the user clicks the FAB.

If your NewApp needs to react to the Canvas being open (for example, to hide copy that shouldn't appear in the submission screenshot), read `PanelOpen` from context — the shell provides it via `provide_context`. Do not re-implement the FAB or the `panel_open` signal inside `app.rs`.

If you rewrite the UI stack off Leptos entirely, reproduce the same split in the replacement: a permanent shell module that owns the four invariant surfaces, and a NewApp content module mounted inside it.

Rules you MUST follow when the iteration app keeps a Feedback affordance (i.e. always):

- **Exactly one trigger.** One FAB, one toolbar button, one keyboard shortcut, one menu item — pick *one* affordance per surface. Do NOT ship "the old Canvas button that no longer works" alongside "a new FAB for the feedback container". If you rewrite, DELETE the previous trigger in the same change. Two buttons where the user can't tell which one is live = broken.
- **One signal drives both.** Bind the Canvas overlay's visibility and the Feedback panel's visibility to the **same** `RwSignal<bool>` (equivalent in whatever stack you're on). When it flips true, both surfaces come up; when it flips false, both go away. No "half-open" state where Canvas is up but Feedback isn't, or vice-versa.
- **Discoverable on every page.** The trigger is visible on every route — floating, pinned, or in a persistent chrome region — never hidden behind a tab switch or a hover-only menu. Icon-only triggers MUST have `aria-label` (and a `title`) so the user knows what they do.
- **Clearly labelled.** The user must be able to tell *at a glance* what that single button does. Iteration zero uses "Send feedback" as the `title` and `aria-label`, a pencil/close icon, and a count badge when there are pending annotations. Keep that intent: the button's label must name the feedback/annotation action explicitly, not just show a glyph.
- **Delete dead triggers.** If you restyle or move the affordance, remove the prior one in the same commit. A deprecated button that "still renders but does nothing" is a regression — the user will click it first, get nothing, and file feedback about *that*.

Concretely: if the user sees two buttons and isn't sure which one opens feedback, you have already failed this invariant. Redesign until there is exactly one.

## Context hygiene — update docs and agents alongside the code

Because future iterations rely on the repo's own documentation for context, any non-trivial change to the app MUST also update:

- `CLAUDE.md` — reflect the new architecture, stack, commands, domain model. Remove stale sections rather than layering on top.
- `.claude/rules/` — update conventions that no longer match the code (or add new ones). Delete rules for layers that no longer exist.
- `.claude/agents/*` — if an agent's description, responsibilities, or tools no longer match the current codebase, update its frontmatter and body. If a whole agent is obsolete, delete it; if the app now needs a new specialist, add one.
- `.claude/skills/*` (if present) — same treatment: keep them accurate or remove them.

The next iteration's agent will read these files first. Leaving them stale is the single biggest way to sabotage iteration N+1.

## Per-iteration dev-server port

This iteration's dev server MUST listen on **port {port}** (base `1530` + iteration `{n}`). The runner has already rewritten `app/src-tauri/tauri.conf.json`, `app/ui/Trunk.toml`, and `app/ui/scripts/trunk-dev.sh` in this worktree to use port `{port}` so concurrent iteration runs don't collide. When the reviewer clicks **Run**, the runner also sets `EVOLVO_ITERATION_PORT={port}` in the child environment.

If you rewrote the stack so the default files no longer exist, you MUST honour `EVOLVO_ITERATION_PORT` in `scripts/run-iteration.sh` (or whatever startup script you ship) and bind the dev/server on that port. Never hardcode `1530` — it belongs to the host Evolvo.

## Keep the iteration runnable — `scripts/run-iteration.sh`

The reviewer UI has a **Run** button that launches the app built in this iteration's worktree. It invokes `scripts/run-iteration.sh` at the worktree root if present, otherwise falls back to `cargo tauri dev` in `app/src-tauri`.

If you rewrite the stack (e.g. move off Tauri/Leptos) you MUST create or update `scripts/run-iteration.sh` so the Run button still works. The script should:

- Start the current app in the foreground (the runner streams its stdout/stderr into a log file).
- Bind the dev/server to `$EVOLVO_ITERATION_PORT` (falling back to `{port}` for this iteration if the env var isn't set).
- Respect `EVOLVO_WORKSPACE_ROOT` if the app stores any state — the runner sets that env var to a per-iteration workspace directory so runs stay isolated from the host Evolvo.
- Exit non-zero on startup failure so the reviewer sees a useful error in the lineage notes.

If you kept the default stack, you can skip the script and rely on the `cargo tauri dev` fallback.

## Verify-before-done — you MUST run the app before calling the task complete

Type-checking is not verification. Before you commit and return, you MUST actually start the app and confirm it boots. The reviewer expects a running binary, not a green `cargo check`.

Concrete steps for the default stack (adapt for whatever stack this iteration ships):

1. Run `cargo check -p evolvo_desktop` and `cargo check -p evolvo_ui --target wasm32-unknown-unknown`. Both must pass.
2. Run `cargo test -p evolvo_desktop` and fix any regression you introduced. Add tests for new host-side logic.
3. Start the app in the background: `EVOLVO_ITERATION_PORT={port} cargo tauri dev` (or `bash scripts/run-iteration.sh`). Wait for the dev server to print its ready line (Trunk prints `server listening at http://127.0.0.1:{port}`). If the build fails or the server doesn't come up, fix the cause — do NOT claim success.
4. Exercise the change: navigate to the affected route, trigger the feedback / canvas / lineage path that the feedback is about, and confirm the user-visible behaviour matches what was asked for. Try to break it — empty inputs, fast clicks, edge cases adjacent to what the feedback described. If any of the four invariants (Feedback Overlay, per-page Canvas overlay, Inbox, Lineage pipeline) regressed, that's a blocker: fix it before finishing.
5. Only after the app actually ran and the change actually worked, commit and return.

If you genuinely cannot run the app in this environment (no display, missing system deps), say so plainly in your final summary — don't fake it. "I couldn't run the app because X" is acceptable; "looks good, tests pass" when you never started the binary is not.

## After implementation — commit, then start the new version

When the change is verified:

1. Stage and commit every file you touched (including updated `CLAUDE.md` / rules / agents). Use a conventional-commit subject like `feat(ui): <short>` or `fix(lineage): <short>`. One focused commit is fine; multiple small commits are better when the work naturally splits.
2. Leave the iteration's app running so the reviewer lands on a live build. If you shut it down earlier to rebuild, start it again before returning: `EVOLVO_ITERATION_PORT={port} cargo tauri dev` (or the equivalent for your stack). The reviewer's Run button will also launch it, but starting it here saves them a click and confirms startup worked.
3. In your final summary mention the port this iteration is serving on ({port}) and how you verified the change.
