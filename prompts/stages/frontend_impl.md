# Stage 4 / 7 — Frontend implementation

{{CONTRACT}}

Build the UI described in `plan.frontend` inside the worktree at
`{{WORKTREE}}`. The backend is already done; do not modify
`app/src-tauri/*`.

## Preconditions (fail fast)

1. `{{PLAN_PATH}}` parses and `frontend.routes` + `frontend.components`
   are non-empty. Otherwise: `STAGE_RESULT: fail: frontend plan missing`.
2. `cargo` is on PATH and the `wasm32-unknown-unknown` target is
   installed (`rustup target list --installed`). Otherwise:
   `STAGE_RESULT: fail: wasm32 target not installed`.
3. `app/ui/src/shell.rs` exists in the worktree (the invariant chrome
   must still be there). Otherwise:
   `STAGE_RESULT: fail: shell.rs missing — prior stage corrupted invariant chrome`.

## Your inputs

1. **Read `{{PLAN_PATH}}`**. `frontend.routes`, `frontend.components`, and
   `backend.commands` are the spec.
2. **Read the canvas PNG** at `{{CANVAS_PNG}}` every time you are choosing
   copy, placement, or a visual affordance. Do not invent — cite regions.

## Files you own

- `app/ui/src/interop.rs` — add a typed `async fn <cmd>(…)` for every new
  `backend.commands[*].name`. Follow the existing pattern: serialize args
  with `serde_wasm_bindgen`, await `invoke(...)`, deserialize the result,
  map errors to `Err(String)`.
- `app/ui/src/app.rs` — rewrite the NewApp content area. `App` must still
  mount `<Shell>` with the NewApp's Home content as its children. Inside
  the shell, every page/route of your new UI must be reachable.
- Additional `app/ui/src/*.rs` modules as needed — one file per component
  cluster. Declare them in `app/ui/src/main.rs` or `app.rs` (whichever
  already hosts the module tree).
- **App-shape scaffold UI** (when `templates.useAppShape` is set): copy
  the scaffold's `ui_app.rs` to the path the manifest's
  `[layout].ui_app` declares (typically
  `app/ui/src/app_shape_<name>.rs`); copy every file in
  `ui_modules/` to its corresponding `[layout].ui_modules[*]` path;
  declare every new module in `app/ui/src/main.rs`. Then rewrite
  `app/ui/src/app.rs` so `App` mounts the scaffold's top-level component
  inside `<Shell>`. Append `styles.css` to `app/ui/styles/app.css` (do
  not replace it). Walk every `// FILL: …` marker in the copied files
  and replace it with the job's code; `// FILL:` left in the diff fails
  this stage.
- `app/ui/src/types.rs` — UI mirrors of any new wire types from the host.
- **`app/ui/styles/app.css`** — this is the ONLY stylesheet you may edit.
  Every new route/component you introduce MUST have matching CSS here:
  layout, spacing, typography, focus states. An unstyled NewApp is a
  regression — earlier builds shipped correct behaviour on a blank-white
  page and it was unshippable. Add rules; do not remove the existing
  Home/landing rules unless your plan replaces that page.

## Must NOT touch

- `app/ui/src/shell.rs` — chrome is invariant.
- `app/ui/src/canvas.rs`, `app/ui/src/feedback_panel.rs`, `voice.rs`,
  `toolbar.rs` — those belong to the feedback overlay and must keep
  working.
- `app/ui/styles.css` — the protected shell layer (app bar, canvas
  overlay, feedback panel, toolbar, lineage pages). Do not edit it.
- `app/ui/styles/tokens.css` — the protected design vocabulary. You may
  read it to see what's available; you may NOT delete or rename a token.
  If a token's *value* genuinely needs to change for this iteration's
  visual identity, that's allowed — but the token NAME must survive.
- `app/ui/styles/base.css` — the protected component primitive library
  (`.btn`, `.btn-primary`, `.btn-danger`, `.btn-ghost`, `.card`, `.input`,
  `.field`, `.field-label`, `.field-hint`, `.empty-state`, `.badge`,
  `.badge-accent`, `.badge-success`, `.badge-danger`). You MAY extend it
  by adding brand-new primitives if genuinely reusable across the app —
  append, never delete or modify an existing rule.

## Design contract — use tokens and primitives

- **Prefer base.css primitives before inventing new classes.** Need a
  button? Use `.btn` / `.btn-primary` / `.btn-danger` / `.btn-ghost`. Need
  a card surface? Use `.card`. A labeled input? `.field` + `.input`. Only
  write a new class in `app/ui/styles/app.css` for something the
  primitives genuinely don't cover (a domain-specific layout, a custom
  component shape).
- **Every value in `app/ui/styles/app.css` must reference a token.** No
  raw hex colors (`#fff`, `#2563eb`, …), no `rgb()`/`rgba()`/`hsl()`
  literals. Use `var(--...)` — e.g. `color: var(--fg)`,
  `background: var(--surface-sunken)`. Need a tinted color? Use
  `color-mix(in srgb, var(--accent) 12%, transparent)` rather than a raw
  rgba.
- **`font-size:` declarations must reference `var(--text-xs|sm|base|lg|xl|2xl)`.**
- **`box-shadow:` declarations must reference `var(--shadow-1|2|3)`** (or
  be `none`).
- **At least one `:hover` rule and one `:focus-visible` rule** must exist
  somewhere in `app/ui/styles/app.css` — every iteration needs at least
  one visible interaction state, ideally on every clickable/focusable
  element you add.
- Token values (not names) are your creative dial — you may reassign
  `--accent` or `--radius` to give this iteration's app its own visual
  identity, as long as every required token name in
  `app/ui/styles/tokens.css` still exists.

## Quality bar

- **No placeholder signals.** Every button that the plan says calls a
  backend command must actually call it via `interop::<cmd>` and reflect
  the response. No `TODO` and no `unimplemented!` in new code; the
  validator greps for those.
- **State survives reload.** Do not hold list state only in a local
  `RwSignal`. Load from the backend on mount, refresh after writes.
  Also refresh when **behaviors** change state on their own: listen for
  the backend's `app://changed` event (via `interop.rs`) or poll the
  affected `list_*` while anything is in flight, and render in-progress
  and failed states — otherwise agents replying, tasks finishing and
  timers firing are invisible and the app reads as a mockup.
- **Accessibility.** Icon-only buttons get `aria-label` + `title`.
- **Feature list every web-sys type you reach for** in
  `app/ui/Cargo.toml`.
- **Styling is not optional.** The validator snapshots
  `app/ui/styles/app.css` at pipeline start and requires the file to grow
  by at least 50 bytes before this stage passes. Write real CSS — class
  selectors used in `app.rs`, layout, readable typography. A growth of
  exactly 0 bytes (you never edited the file) fails the stage.

## Validation this stage will face

`validate_frontend_impl` checks:

- For every backend command, `app/ui/src/interop.rs` references the
  command's name literal.
- For every component `name`, some `.rs` file under `app/ui/src/` contains
  `fn <name>`.
- No stub smells in `app.rs`.
- `app/ui/styles/app.css` exists and has grown by ≥ 50 bytes vs the
  snapshot taken at pipeline start.
- **Design lint on `app/ui/styles/app.css`** (FAIL lines cite the exact
  line number and offending value):
  - No hex color literals (`#abc`, `#aabbcc`, …) in value position.
  - No `rgb(`/`rgba(`/`hsl(` literals.
  - Every `font-size:` references `var(--text-*)`.
  - Every `box-shadow:` references `var(--shadow-*)` (or is `none`).
  - At least one `:hover` rule and one `:focus-visible` rule exist.
  - CSS comments are skipped when scanning — a hex value mentioned only
    in a comment does not fail the lint.
- **`app/ui/styles/tokens.css` token integrity** — every token name in
  the protected vocabulary must still be defined (values may change,
  names may not).
- `cargo check -p evolvo_ui --target wasm32-unknown-unknown` exits 0.

Run `evolvo_desktop validate --stage frontend_impl --job-dir <dir>
--worktree <dir>` yourself before exiting — iterate until every check is
green (see `prompts/stages/_self_validate.md`).

## When finished

1. Make sure `cargo check -p evolvo_ui --target wasm32-unknown-unknown`
   passes. If it does not and you cannot fix it inside this stage's
   scope, reply `STAGE_RESULT: fail: wasm check failed: <one-line cause>`.
2. Otherwise reply per the Operating contract — final line
   `STAGE_RESULT: ok`.
