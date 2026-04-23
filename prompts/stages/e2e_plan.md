# Stage 5 / 7 — End-to-end plan

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

- `scenarios[]` — each `{ id, title, steps[], motivatedByRegions[] }`:
  - `id`: short snake_case (`happy_path_crud`, `delete_restores_empty_state`,
    `persistence_after_reload`).
  - `steps[]`: ordered imperative instructions a human (or the next
    stage's agent running the app) can execute verbatim — "Click 'New
    task'", "Type 'Buy milk'", "Press Enter", "Expect the list to contain
    'Buy milk'". No pseudocode, no "test that".
  - Cover at least: (a) the happy CRUD path, (b) one edge case, (c) one
    cross-region interaction (if >1 region).

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
        "Launch the app at the iteration's port.",
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

Save and reply `e2e_plan done`.
