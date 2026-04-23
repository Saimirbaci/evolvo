# Stage 2 / 7 — Backend implementation

You are implementing the backend plan committed to `{{PLAN_PATH}}` for
Evolvo iteration `{{ITERATION}}` (job `{{JOB_ID}}`). The BackendPlan stage
has already written the plan; your job is to make it real in the worktree
at `{{WORKTREE}}`.

## Your inputs

1. **Read `{{PLAN_PATH}}` first**. Treat `backend.entities`,
   `backend.commands`, `backend.tests`, `backend.storage`, and
   `templates.useTemplates` as the spec. Do not improvise new commands; if
   the plan is wrong, note it in `history` but implement what it says and
   let the validator catch drift.
2. **Read the canvas PNG** at `{{CANVAS_PNG}}` whenever you are making a
   judgment call about data shape or field meaning. No paraphrase.
3. **Read `{{REGION_INDEX}}`** so that every function you add can trace
   back to a region id via the plan's `motivatedByRegions` lists.

## What you must produce in `{{WORKTREE}}`

- **Types**: add each planned entity to `app/src-tauri/src/types.rs` (or a
  new module) with `#[derive(Debug, Clone, Serialize, Deserialize,
  PartialEq)]` and `#[serde(rename_all = "camelCase")]`. Use `#[serde(default)]`
  on optional fields — forward-compat matters.
- **Storage**: extend `app/src-tauri/src/store.rs` with `save_<entity>`,
  `load_<entity>`, `list_<entities>`, `delete_<entity>` backed by JSON files
  under the entity directory named in `backend.storage.path`. Use
  `sanitise_filename` for any user-provided id. No in-memory-only stores.
- **Commands**: every `backend.commands[*].name` becomes a
  `#[tauri::command] fn <name>(...) -> Result<T, String>` in
  `app/src-tauri/src/commands.rs`, thin-wrapping a plain `_impl` function
  that takes `&Store` so tests can hit the logic without the macro.
- **Registration**: add every new command to the `invoke_handler!` list in
  `app/src-tauri/src/lib.rs` (and/or `main.rs`). This is the single most
  common regression — grep for the new names after you add them.
- **Templates**: for every name in `templates.useTemplates`, follow
  `templates/integrations/<name>/README.md` — copy `host.rs` / `ui.rs` into
  place, merge `deps.toml`, and register the commands. Do not hand-roll
  what a template already ships.
- **Tests**: add each planned test by its exact name as a `#[test] fn
  <name>` in the module the plan names. Use `tempfile::tempdir()`;
  `EVOLVO_WORKSPACE_ROOT` must not leak into real disk.

## Zero-tolerance stub smells

The validator will grep your diff for `TODO`, `unimplemented!`, and
`todo!()`. Any of those in new code fails the stage. If you cannot
implement something inside the iteration budget, shrink the plan (edit
`{{PLAN_PATH}}` to remove the command), then implement the reduced set
fully.

## Validation this stage will face

The `validate_backend_impl` validator runs after you finish and checks:

- Every planned command has a `fn <name>` in `commands.rs`.
- Every planned command appears as `commands::<name>` in the invoke
  handler registration.
- Every planned test exists as a `fn <name>` somewhere in the host crate.
- `cargo check -p evolvo_desktop` and `cargo test -p evolvo_desktop --lib`
  both exit 0.
- No stub smells in `commands.rs`.

Run these yourself before you exit:

```bash
cargo check -p evolvo_desktop
cargo test  -p evolvo_desktop --lib
```

## Golden example

For the todo-app plan from the BackendPlan stage's example, the diff
touches:

- `app/src-tauri/src/types.rs`: add `Task`, `CreateTaskPayload`.
- `app/src-tauri/src/store.rs`: add `save_task`, `load_task`,
  `list_tasks`, `delete_task` using
  `workspace_root.join("tasks").join(format!("{id}.json"))`.
- `app/src-tauri/src/commands.rs`: add `create_task`, `list_tasks`,
  `toggle_task`, `delete_task`, each with an `_impl(&Store, …)` counterpart.
- `app/src-tauri/src/lib.rs`: extend `invoke_handler![..., commands::create_task,
  commands::list_tasks, commands::toggle_task, commands::delete_task]`.
- Three `#[test]` functions with the exact names from the plan.

## When finished

1. Make sure `cargo check` and `cargo test` are green.
2. Reply in chat with ONLY `backend_impl done`. The validator reads the
   worktree, not your reply.
