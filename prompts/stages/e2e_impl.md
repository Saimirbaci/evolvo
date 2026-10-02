# Stage 6 / 7 — End-to-end implementation

{{CONTRACT}}

Prove the plan's scenarios pass on the real running app inside
`{{WORKTREE}}`.

## Preconditions (fail fast)

1. `{{PLAN_PATH}}` parses and `e2e.scenarios` is non-empty AND
   `e2e.persistenceSmoke` is present. Otherwise:
   `STAGE_RESULT: fail: e2e plan missing scenarios or persistence smoke`.
2. `scripts/run-iteration.sh` exists OR `cargo tauri dev` is runnable.
   Otherwise: `STAGE_RESULT: fail: no startup path for the app`.
3. Port `{{PORT}}` is free (`lsof -iTCP:{{PORT}} -sTCP:LISTEN` returns
   nothing). If it's bound by a stale process, do NOT kill it blindly —
   fail with `STAGE_RESULT: fail: iteration port busy <pid>` and let
   the reviewer triage.

## Your inputs

1. **Read `{{PLAN_PATH}}`** — `e2e.scenarios` and `e2e.persistenceSmoke`
   are your verification contract.
2. **Canvas + regions** — only needed if a scenario is ambiguous; prefer
   executing what the plan literally says.

## What you must do

1. Start the app on port `{{PORT}}` using `scripts/run-iteration.sh` if
   it exists, else `cargo tauri dev` from `app/src-tauri`. Wait for
   Trunk to print `server listening at http://127.0.0.1:{{PORT}}`.
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
5. For scenarios with `verifiesBehaviors`, the pass condition is the
   **effect** happening without you causing it directly (a reply from
   the agent, a task reaching done, a timer-driven update). If the effect
   never shows, the behavior is broken or not wired — fix the backend or
   the UI refresh path; never satisfy the step by creating the expected
   record yourself or by seeding it. For `llm` behaviors, a failure from
   the model CLI (e.g. `claude` not installed / not logged in) must be
   surfaced in the UI as an error state, and recorded in history.

## Do not

- Do not stub the scenarios by adding a unit test named after them — they
  are live-app verifications, not Rust tests. (Rust tests belong to
  BackendImpl.)
- Do not remove a scenario to make the validator pass.
- Do not skip the persistence smoke; every NewApp must survive a restart.

## When finished

Save the plan (with history entries), leave the dev-server running, and
reply per the Operating contract. Include `port: {{PORT}}` in your
summary. The final line must be `STAGE_RESULT: ok` if every scenario
passed (including the persistence smoke) — otherwise
`STAGE_RESULT: fail: <which scenario failed and why, in one line>`.
