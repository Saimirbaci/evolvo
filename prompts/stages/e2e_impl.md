# Stage 6 / 7 — End-to-end implementation

Prove the plan's scenarios pass on the real running app inside
`{{WORKTREE}}`.

## Your inputs

1. **Read `{{PLAN_PATH}}`** — `e2e.scenarios` and `e2e.persistenceSmoke`
   are your verification contract.
2. **Canvas + regions** — only needed if a scenario is ambiguous; prefer
   executing what the plan literally says.

## What you must do

1. Start the app on the iteration's port using
   `scripts/run-iteration.sh` if it exists, else `cargo tauri dev` from
   `app/src-tauri`. Wait for Trunk to print `server listening at
   http://127.0.0.1:<port>`.
2. For each scenario in `plan.e2e.scenarios`, execute its `steps[]`
   verbatim and record the outcome in `plan.history` as
   `{ stage: "e2e_impl", kind: "note", message: "<id>: pass" }` or
   `{ ..., message: "<id>: fail - <observation>" }`.
3. Run the persistence smoke: create an instance of
   `persistenceSmoke.entity`, kill the dev-server, confirm the JSON file
   at `<EVOLVO_WORKSPACE_ROOT>/<expectedDirectory>/<id>.json` exists, then
   relaunch and confirm the entity still shows in the UI.
4. If any scenario fails, **fix the app** (edit code in the worktree),
   re-run, and update history. Do not rewrite the scenario to match
   broken behaviour.

## Do not

- Do not stub the scenarios by adding a unit test named after them — they
  are live-app verifications, not Rust tests. (Rust tests belong to
  BackendImpl.)
- Do not remove a scenario to make the validator pass.
- Do not skip the persistence smoke; every NewApp must survive a restart.

## When finished

Save the plan (with history entries), leave the dev-server running, and
reply `e2e_impl done` with the iteration port number on the next line.
