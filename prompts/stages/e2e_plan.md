# Stage 5 / 7 — End-to-end plan

{{CONTRACT}}

## Preconditions (fail fast)

1. `{{PLAN_PATH}}` parses and `backend.commands`, `frontend.routes`,
   `frontend.components` are all non-empty. Otherwise:
   `STAGE_RESULT: fail: prior stages incomplete — cannot plan e2e`.

Backend + frontend exist. Your job: write the E2E verification contract
into `plan.e2e` so the next stage (and the final review) can prove the
app actually works from the user's point of view.

## Your inputs

1. **Read `{{PLAN_PATH}}`** — `backend.commands`, `frontend.routes`, and
   `frontend.components` are what the scenarios must exercise.
2. **Read the canvas PNG** `{{CANVAS_PNG}}` + `{{REGION_INDEX}}`. Every
   region should show up in at least one scenario's `motivatedByRegions`.

## What you write

In `{{PLAN_PATH}}` under `e2e`:

- `scenarios[]` — each `{ id, title, steps[], motivatedByRegions[], verifiesBehaviors[] }`:
  - `id`: short snake_case (`happy_path_crud`, `delete_restores_empty_state`,
    `persistence_after_reload`).
  - `steps[]`: ordered imperative instructions a human (or the next
    stage's agent running the app) can execute verbatim — "Click 'New
    task'", "Type 'Buy milk'", "Press Enter", "Expect the list to contain
    'Buy milk'". No pseudocode, no "test that".
  - Cover at least: (a) the happy CRUD path, (b) one edge case, (c) one
    cross-region interaction (if >1 region).
  - **Cover every `backend.behaviors[]` entry** by listing its `name` in
    some scenario's `verifiesBehaviors`. That scenario's steps must assert
    the behavior's *effect* — something the user did NOT type themselves:
    "Send 'Summarise the spec' to Coder" → "Expect a new message *from*
    Coder in the message bus within 60s"; "Assign 'Write tests' to Tester"
    → "Expect the task to reach Done and show a result". Asserting that the
    user's own input was stored does not verify a behavior. `llm`
    behaviors call a real model, so give them a generous wait (≤ 120s)
    and assert on presence/shape, not exact wording. The validator rejects
    a plan where any behavior is uncovered.

- `persistenceSmoke` — `{ entity, expectedDirectory }`: the smoke test
  closes the app after creating an entity and reopens it, expecting the
  JSON file at `<EVOLVO_WORKSPACE_ROOT>/<expectedDirectory>/<id>.json` to
  still be there. This is mandatory — every NewApp must prove it actually
  writes to disk.

## Golden example

```json
{
  "scenarios": [
    {
      "id": "happy_path_crud",
      "title": "Create, toggle, delete a task",
      "steps": [
        "Launch the app at port {{PORT}}.",
        "Type 'Buy milk' in the new-task input and press Enter.",
        "Expect a row 'Buy milk' in the task list.",
        "Click the checkbox on the 'Buy milk' row.",
        "Expect the row to render with a strike-through style.",
        "Click the trash icon on the 'Buy milk' row.",
        "Expect the list to no longer contain 'Buy milk'."
      ],
      "motivatedByRegions": ["R1"]
    },
    {
      "id": "persistence_across_reload",
      "title": "A created task survives app restart",
      "steps": [
        "Launch the app, create 'Persist me', close the window.",
        "Re-launch the app.",
        "Expect 'Persist me' in the list."
      ],
      "motivatedByRegions": ["R1"]
    }
  ],
  "persistenceSmoke": {"entity": "Task", "expectedDirectory": "tasks"}
}
```

## When finished

Save and reply per the Operating contract — final line `STAGE_RESULT: ok`
(or `STAGE_RESULT: fail: <reason>`).
