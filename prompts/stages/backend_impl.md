# Stage 2 / 7 — Backend implementation

{{CONTRACT}}

You are implementing the backend plan committed to `{{PLAN_PATH}}` for
Evolvo job `{{JOB_ID}}`. The BackendPlan stage has already written the
plan; your job is to make it real in the worktree at `{{WORKTREE}}`.

## Preconditions (fail fast)

1. `{{PLAN_PATH}}` exists, parses, and has a non-empty
   `backend.commands` array. Otherwise:
   `STAGE_RESULT: fail: backend plan missing or empty`.
2. `cargo` and `git` are on PATH. Otherwise: `STAGE_RESULT: fail: cargo not on PATH`.
3. The worktree is clean of unrelated edits (`git status` shows only
   files this stage will touch). If a prior stage left dirty state,
   continue — but note it in `plan.history`.

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
- **Integration templates**: for every name in `templates.useTemplates`,
  follow `templates/integrations/<name>/README.md` — copy `host.rs` /
  `ui.rs` into place, merge `deps.toml`, and register the commands. Do
  not hand-roll what a template already ships.
- **App-shape template** (when `templates.useAppShape` is set): follow
  `templates/app_shapes/<name>/SKILL.md` end-to-end. In short:
  1. Copy `host.rs` to the path declared in the manifest's
     `[layout].host_module` (typically
     `app/src-tauri/src/app_shapes/<name>.rs`). Add
     `pub mod app_shapes;` and `pub mod <name>;` to the appropriate
     `mod.rs` files (create `app_shapes/mod.rs` if it doesn't exist).
  2. Register every `#[tauri::command]` from the scaffold's `host.rs` in
     `invoke_handler!` (lib.rs). Grep for the command names afterwards
     to confirm.
  3. Merge `deps.toml` if any optional renderer / dependency is used.
  4. Walk every `// FILL: …` marker in the copied files and replace it
     with job-specific code based on the user's request and the
     plan's `entities` list. The validator (planned) refuses any `// FILL:`
     surviving in the final diff.
  5. If `templates.backendBindings.<name>` is set AND the integration's
     `seam_<name>.rs` adapter exists, replace the scaffold's default
     `*Store` impl factory (`build_store`) with the seam's adapter. If
     the seam isn't shipped yet, fall back to the default JSON impl and
     append a `history` entry on `plan.json` recording the missing seam
     — never silently bake in a half-implemented adapter.
  6. Tests: reuse the scaffold's existing `#[cfg(test)] mod tests`
     verbatim. The plan's `tests[]` should already name those test
     functions; if it doesn't, add them.
- **Tests**: add each planned test by its exact name as a `#[test] fn
  <name>` in the module the plan names. Use `tempfile::tempdir()`;
  `EVOLVO_WORKSPACE_ROOT` must not leak into real disk.

## Zero-tolerance stub smells

The validator will grep your diff for **the full set of stub
shapes** — not just the ones Claude knows to avoid by default. Each
of these will fail the stage:

- `TODO`, `unimplemented!`, `todo!()` (historic)
- `Err("not impl…")`, `Err("nyi…")`, `Err("todo…")`, `Err("not yet…")`
- `panic!("not impl…")`, `panic!("nyi…")`, `panic!("not yet…")`
- A **trailing** empty default at the end of a function body:
  `Ok(Vec::new())`, `Ok(vec![])`, `Ok(Default::default())`,
  `Ok(HashMap::new())`, `Ok(BTreeMap::new())`, `Ok(HashSet::new())`
  immediately followed by `}`. If you legitimately need to return
  an empty container, route through `state.store().list_*()` — the
  empty case is the store's job, not the command's.

If you cannot implement something inside this job's budget, shrink
the plan (edit `{{PLAN_PATH}}` to remove the command), then
implement the reduced set fully.

## Anchor contract — the plan tells you what each body must mention

Every command in `plan.backend.commands` carries a `bodySketch` (and
optionally explicit `anchors`). The validator extracts identifier-
shaped tokens from these and asserts at least one appears in your
implemented function body. Examples:

- A command with `anchors: ["save_task", "current_time_unix_ms"]`
  must mention at least one of those identifiers in the body. If your
  body says `Ok(Task::default())` and never calls `save_task`, the
  validator fails the stage.
- A command with only a `bodySketch` (no explicit anchors) has its
  anchors derived from the sketch — typically the store method names
  and helper function names the planner referenced.

This is the contract between plan and implementation. **Don't fake
it**: if the plan's `bodySketch` says "call store.save_task", your
body must actually call `save_task`. Naming the function correctly
is no longer enough.

## Persistence layer — every entity needs CRUD in store.rs

For each entity in `plan.backend.entities`, the validator requires at
least 3 of the 4 CRUD verbs to be implemented in the host crate
(typically in `app/src-tauri/src/store.rs`, but any module is fine):

- `fn save_<entity>` / `fn save_<entity_plural>`
- `fn load_<entity>`
- `fn list_<entity_plural>` (or `fn list_<entity>` as a tolerated alias)
- `fn delete_<entity>`

This is what prevents the "command returns fake data, never wrote
to disk" failure mode. A command that just builds a default value
and returns it passes neither the anchor check nor this CRUD
check — surface area is properly bounded.

`// FILL: …` markers from app-shape scaffolds count as stub smells once
the scaffold has been copied in: every `FILL` you do not legitimately
delete (because it doesn't apply to this iteration) you MUST replace
with real code. Leaving a `// FILL: …` comment in the diff fails the
stage the same way `TODO` does.

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

1. Make sure `cargo check` and `cargo test` are green. If either fails
   and you cannot fix it inside this stage's scope, reply
   `STAGE_RESULT: fail: cargo check|test failed: <one-line cause>`.
2. Otherwise reply per the Operating contract — a short summary if
   useful, then a final line `STAGE_RESULT: ok`. The validator reads
   the worktree; the sentinel is the runtime's signal.
