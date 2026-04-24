# Stage 4 / 7 — Frontend implementation

Build the UI described in `plan.frontend` inside the worktree at
`{{WORKTREE}}`. The backend is already done; do not modify
`app/src-tauri/*`.

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
- `app/ui/src/types.rs` — UI mirrors of any new wire types from the host.
- **`app/ui/styles.css`** — Trunk bundles this single stylesheet. Every new
  route/component you introduce MUST have matching CSS here: layout,
  spacing, typography, focus states. An unstyled NewApp is a regression —
  previous iterations shipped correct behaviour on a blank-white page and
  it was unshippable. Add rules, do not remove existing shell/canvas CSS.

## Must NOT touch

- `app/ui/src/shell.rs` — chrome is invariant.
- `app/ui/src/canvas.rs`, `app/ui/src/feedback_panel.rs`, `voice.rs`,
  `toolbar.rs` — those belong to the feedback overlay and must keep
  working.

## Quality bar

- **No placeholder signals.** Every button that the plan says calls a
  backend command must actually call it via `interop::<cmd>` and reflect
  the response. No `TODO` and no `unimplemented!` in new code; the
  validator greps for those.
- **State survives reload.** Do not hold list state only in a local
  `RwSignal`. Load from the backend on mount, refresh after writes.
- **Accessibility.** Icon-only buttons get `aria-label` + `title`.
- **Feature list every web-sys type you reach for** in
  `app/ui/Cargo.toml`.
- **Styling is not optional.** The validator snapshots `app/ui/styles.css`
  at pipeline start and requires the file to grow by at least 50 bytes
  before this stage passes. Write real CSS — class selectors used in
  `app.rs`, layout, readable typography. A growth of exactly 0 bytes (you
  never edited the file) fails the stage.

## Validation this stage will face

`validate_frontend_impl` checks:

- For every backend command, `app/ui/src/interop.rs` references the
  command's name literal.
- For every component `name`, some `.rs` file under `app/ui/src/` contains
  `fn <name>`.
- No stub smells in `app.rs`.
- `app/ui/styles.css` exists and has grown by ≥ 50 bytes vs the snapshot
  taken at pipeline start.
- `cargo check -p evolvo_ui --target wasm32-unknown-unknown` exits 0.

Run the check yourself before exiting.

## When finished

1. Make sure `cargo check -p evolvo_ui --target wasm32-unknown-unknown`
   passes.
2. Reply `frontend_impl done`.
