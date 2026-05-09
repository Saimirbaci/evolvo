# App-shape templates

A flat, tag-indexed library of **whole-app scaffolds** the NewApp planner can
pick *one* of when an iteration's user request matches a recognisable shape
(an editor, a board, a tabular grid, …). Each scaffold is a working
Tauri + Leptos NewApp body that mounts inside the invariant `<Shell>`
(`app/ui/src/shell.rs`) and ships:

- a `manifest.toml` declaring its name, tags, **data interface**, and
  pre-wired backend seams,
- a `SKILL.md` describing the contract — what is in the box, what blanks
  the agent fills, where the backend seams plug in,
- a `host.rs` (Tauri commands + a `*Store` trait + a default JSON
  implementation that uses the existing host `Store`),
- one or more UI files (`ui_app.rs` + supporting `.rs` modules) the
  implementer copies into `app/ui/src/`,
- `styles.css` deltas to append to `app/ui/styles.css`,
- `deps.toml` with any new Cargo dependencies to merge.

This sits **next to**, not instead of, `templates/integrations/`. App-shape
templates define the *shape* of the app (what it persists, what UI affords
that data); integration templates plug *capabilities* into that shape
(sqlite, REST, Google OAuth, …). The two compose: an app-shape declares
which integration templates can back its data interface, and the planner
records the chosen binding in `plan.json`'s `templates.backendBindings`.

## How the planner uses these (Stage 1 / 7 — BackendPlan)

1. Read this index plus each shortlisted scaffold's `manifest.toml` and
   the **first 40 lines of `SKILL.md`**. Don't read every full SKILL.md —
   the manifest summary is enough to decide.
2. Pick **at most one** app-shape. If nothing in the library matches, pick
   none — building a NewApp from scratch (`HomePage` rewrite) is still the
   default. Don't force a poor fit.
3. Set `plan.templates.useAppShape` to the name (or leave unset / null).
4. Independently of (3), set `plan.templates.useTemplates` to the list of
   integration templates the app needs (`openrouter`, `sqlite`, …).
5. If the chosen app-shape's `manifest.toml` advertises a `[backend_seams]`
   section AND one of those seams matches an integration you already
   picked, record the binding in `plan.templates.backendBindings`:

   ```json
   "templates": {
     "useAppShape": "markdown_editor",
     "useTemplates": ["sqlite"],
     "backendBindings": { "markdown_editor": "sqlite" }
   }
   ```

   That tells the BackendImpl stage to use the integration's adapter
   instead of the scaffold's default JSON implementation. If no binding is
   recorded, the scaffold ships with its default backend (the host `Store`
   writing JSON-per-entity under the workspace root).

## How the implementer uses these (Stages 2 + 4 — BackendImpl, FrontendImpl)

For every name in `plan.templates.useAppShape`, follow that scaffold's
`SKILL.md` end-to-end:

1. Copy `host.rs` to `app/src-tauri/src/app_shapes/<name>.rs`, register
   every `#[tauri::command]` in `app/src-tauri/src/lib.rs`'s
   `invoke_handler!`.
2. Copy `ui_app.rs` and any `ui_modules/*.rs` into `app/ui/src/`,
   declaring the new modules in `app/ui/src/main.rs`.
3. Append the scaffold's `styles.css` to `app/ui/styles.css`.
4. Merge `deps.toml` into `Cargo.toml` (workspace deps) and the relevant
   crate `[dependencies]` block.
5. Walk every `// FILL: …` marker in the copied files and replace it
   with the iteration-specific code requested by the user. Anything
   marked `FILL` left in the diff fails the validator.
6. If `plan.templates.backendBindings[<this-shape>]` is set, the
   integration template's `seam_<name>.rs` (when present) replaces the
   scaffold's default `*Store` implementation. The scaffold's UI is
   unchanged either way — only the persistence backend differs.

The scaffold's UI mounts inside `<Shell>` automatically: rewrite
`app/ui/src/app.rs` so its `App` component still wraps the new content in
`<Shell>` (the four product invariants in `.claude/rules/common/product-invariants.md`
hold by construction).

## Available scaffolds

| Name                | Tags                                        | One-liner                                                                                                       |
|---------------------|---------------------------------------------|-----------------------------------------------------------------------------------------------------------------|
| `markdown_editor`   | `text-editing`, `single-document`, `writing` | Two-pane markdown editor: document list, source pane, rendered preview. Pluggable `DocumentStore`.              |
| `kanban_board`      | `task-tracking`, `board`, `drag-and-drop`   | Three-column kanban (Todo / Doing / Done) with draggable cards. Pluggable `BoardStore`.                          |
| `data_grid`         | `tabular`, `crud`, `filterable`             | Sortable, filterable rows-and-columns grid with inline editing. Pluggable `RowStore`, schemaless rows.          |

## Authoring a new app-shape

See [`SCHEMA.md`](./SCHEMA.md) for the contract every scaffold must
satisfy. New scaffolds are accepted when:

- `manifest.toml` parses against the schema,
- `SKILL.md` lists every `// FILL:` marker present in the scaffold's code
  *and* every backend seam the manifest claims,
- the scaffold's code, dropped verbatim into a fresh worktree and given
  trivially-filled blanks, makes `cargo check --workspace` and
  `cargo check -p evolvo_ui --target wasm32-unknown-unknown` exit 0,
- mounting it inside `<Shell>` does not regress I-P1 through I-P4 (Lineage,
  Feedback Overlay, per-page Canvas overlay, single FAB, lineage
  portability).

## Why "flat with tags" instead of a strict decision tree

Trees go stale the moment a user asks for a hybrid that doesn't sit on a
single path ("spreadsheet but graph-shaped"). The planner already reads
prose; matching on tag overlap (`tags ∩ user_request_keywords`) is more
forgiving and doesn't require maintaining a tree topology in lockstep with
the catalogue. If the catalogue grows past ~20 scaffolds and the planner
starts misclassifying, we can always add a coarse `category` field on top
of `tags` later — the manifest is forward-compatible.
