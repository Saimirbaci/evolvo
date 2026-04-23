# Stage 3 / 7 — Frontend plan

Backend is real: types, storage, Tauri commands, and host tests exist in
`{{WORKTREE}}`. Your job is to plan the Leptos UI that drives it.

## Your inputs

1. **Read `{{PLAN_PATH}}`**. The `backend.commands[*].name` list is the set
   of primitives available to the frontend — you may only reference these.
2. **Read the canvas PNG** at `{{CANVAS_PNG}}`. Every region in
   `{{REGION_INDEX}}` is a claim the user made about what the UI should
   look / behave like. Treat the drawing as the layout brief.
3. Skim `app/ui/src/shell.rs` (invariant chrome — don't replan it) and
   `app/ui/src/app.rs` (the `NewApp` content area you will rewrite).

## What goes in the plan

Mutate `{{PLAN_PATH}}` to fill `frontend`:

- `routes[]`: ordered list of routes. Each is `{ path, component,
  usesCommands[], motivatedByRegions[] }`. The first route should be `/`
  (the Home of the NewApp). Icon-only drawings still need an `aria-label`
  — if the canvas shows icons, note the intended label in the route's
  component or in `plan.history`.
- `components[]`: every Leptos component function you expect to write.
  Each is `{ name, module, usesCommands[], summary, motivatedByRegions[]
  }`. `name` must match the Rust `fn <name>(...) -> impl IntoView` (no
  suffix, no prefix — the validator greps for exactly `fn <name>`).
- `budget`: keep `{minRoutes: 1, minComponents: 2}` unless the plan
  deliberately wants more.

## The hard constraint

Every string in any `usesCommands[]` MUST already appear in
`plan.backend.commands[*].name`. If you need a command the backend didn't
plan, **go back and add it to `backend.commands`** in this same edit —
the backend section is not frozen. The validator refuses unknown command
references.

## Golden example

For the todo-app spec:

```json
{
  "routes": [
    {
      "path": "/",
      "component": "TaskListPage",
      "usesCommands": ["list_tasks", "create_task", "toggle_task", "delete_task"],
      "motivatedByRegions": ["R1"]
    }
  ],
  "components": [
    {
      "name": "TaskListPage",
      "module": "app.rs",
      "usesCommands": ["list_tasks", "create_task", "toggle_task", "delete_task"],
      "summary": "Full-page CRUD: header with input, list of tasks, each row has toggle + delete.",
      "motivatedByRegions": ["R1"]
    },
    {
      "name": "TaskRow",
      "module": "app.rs",
      "usesCommands": ["toggle_task", "delete_task"],
      "summary": "Single row: checkbox bound to `toggle_task`, trash icon bound to `delete_task`.",
      "motivatedByRegions": ["R1"]
    }
  ],
  "budget": {"minRoutes": 1, "minComponents": 2}
}
```

## When finished

1. Save the plan.
2. Reply in chat with ONLY `frontend_plan done`.
