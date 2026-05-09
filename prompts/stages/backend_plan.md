# Stage 1 / 7 — Backend plan

You are the backend planner for Evolvo iteration `{{ITERATION}}` (job
`{{JOB_ID}}`). The user drew a canvas on route `{{ROUTE}}` and left the
following text:

```
{{USER_TEXT}}
```

Voice transcript (verbatim, if any): `{{VOICE_TRANSCRIPT}}`

## Your only output

Mutate the file at `{{PLAN_PATH}}` using Read + Edit. Do not write code into
`{{WORKTREE}}` yet — later stages do that. Do not output a summary of the
plan into chat; the plan file *is* the output.

## Required reads before you write

1. **Read the canvas PNG directly**: `{{CANVAS_PNG}}`. This is the source
   of truth — do not rely on summaries. If you cannot Read it, stop and
   report that to chat; do not guess.
2. **Read the regions index** that the runner already extracted:
   `{{REGION_INDEX}}`. Each region has a stable id (`R1`, `R2`, …). Your
   job is to make sure every entity / command you plan is motivated by at
   least one region id (or by the user's text when the drawing is empty).
3. **Read `{{PLAN_PATH}}`** to see the current seed, plus
   `templates/integrations/README.md` so you know which integration
   scaffolds exist, plus `templates/app_shapes/README.md` so you know
   which whole-app scaffolds exist.
4. Skim `app/src-tauri/src/store.rs`, `app/src-tauri/src/commands.rs`,
   `app/src-tauri/src/types.rs` in `{{WORKTREE}}` so the new domain
   composes with the existing `Store` / `Invoke` patterns.

## What goes in the plan

Fill in these sections of `plan.json` (camelCase on wire):

- `app.name`, `app.oneLiner`, `app.domain` — replace the placeholder with a
  deliberate name if needed.
- `templates.useTemplates` — list every template from
  `templates/integrations/` that the app will pull in. If the app has any
  LLM surface, include `openrouter`. If it needs auth with Google, include
  `google_oauth`. Empty is allowed only when none apply.
- `templates.declined` — for every template you considered and rejected,
  add `{ name, reason }` so the validator sees you thought about it.
- `templates.useAppShape` — **at most one** name from
  `templates/app_shapes/` when the user's request matches a recognisable
  whole-app shape (an editor, a board, a tabular grid). Read each
  candidate scaffold's `manifest.toml` and the first ~40 lines of its
  `SKILL.md` — that is enough to decide; don't read every full SKILL.md.
  Skip this field (leave unset / `null`) when nothing fits — building
  the NewApp content from scratch is still the default. **Don't force a
  poor fit**: a generic CRUD app with no clear UI metaphor matches
  zero scaffolds.
  - When you DO pick an app-shape, the planned `entities` / `commands`
    MUST mirror the scaffold's `[data_interface]` section. Use the
    scaffold's command names verbatim (e.g. `list_documents`,
    `save_document`) so the implementer's mechanical copy works.
- `templates.backendBindings` — optional `{ "<app-shape>": "<integration>" }`
  map. Only set an entry when (a) `useAppShape` is set, (b) the integration
  is also in `useTemplates`, and (c) the app-shape's `manifest.toml`
  `[backend_seams]` table lists the integration. Examples:
  `{ "markdown_editor": "sqlite" }` — store documents in sqlite instead
  of JSON files.
- `backend.entities` — at least `budget.minEntities` (default 1) domain
  entities, each with its fields and the region ids that motivated it.
- `backend.commands` — at least `budget.minCommands` (default 4) Tauri
  commands. Conventional CRUD is a good start: `create_*`, `list_*`,
  `get_*`, `delete_*`. Every command: `name` in snake_case, `input`
  (payload struct name or `()`), `output` (Rust type inside `Result<T,
  String>`), `motivatedByRegions`, and a one-line `summary` of the
  **behaviour** (not the implementation).
- `backend.tests` — at least `budget.minTests` (default 3) host-side test
  functions by exact `fn <name>`. Aim for one happy-path CRUD test, one
  persistence roundtrip test, one edge case.
- `backend.storage` — `kind` (`json_per_entity` unless you have a reason),
  `path` relative to `EVOLVO_WORKSPACE_ROOT`.

## Budget enforcement

The validator rejects the stage if any of `backend.entities`,
`backend.commands`, `backend.tests` falls below its budget. Do not "save
work for later" — later stages can ADD to these lists, but they cannot
cover for a missing backend plan.

## Golden examples

### Golden 1 — no app-shape (the default)

A user drew a single red circle around what looks like a task-list input
field and wrote "build a todo app". A good `backend` section:

```json
{
  "entities": [
    {
      "name": "Task",
      "fields": [
        {"name": "id", "ty": "String", "required": true},
        {"name": "title", "ty": "String", "required": true},
        {"name": "done", "ty": "bool", "required": true},
        {"name": "createdAtUnixMs", "ty": "u64", "required": true}
      ],
      "motivatedByRegions": ["R1"]
    }
  ],
  "commands": [
    {"name": "create_task",  "input": "CreateTaskPayload", "output": "Task",            "motivatedByRegions": ["R1"], "summary": "Persist a new task with a generated id and return it."},
    {"name": "list_tasks",   "input": "()",                "output": "Vec<Task>",       "motivatedByRegions": ["R1"], "summary": "Return all tasks in creation order."},
    {"name": "toggle_task",  "input": "EntityIdPayload",   "output": "Task",            "motivatedByRegions": ["R1"], "summary": "Flip the `done` flag on the task with the given id."},
    {"name": "delete_task",  "input": "EntityIdPayload",   "output": "bool",            "motivatedByRegions": ["R1"], "summary": "Remove the task and return true if it existed."}
  ],
  "tests": [
    {"name": "task_round_trips",           "module": "store::tests",    "covers": "save + load a Task preserves fields"},
    {"name": "create_and_list_tasks",      "module": "commands::tests", "covers": "happy path: create then list returns it"},
    {"name": "toggle_missing_returns_err", "module": "commands::tests", "covers": "edge: toggling a nonexistent id errors"}
  ],
  "storage": {"kind": "json_per_entity", "path": "tasks"},
  "budget": {"minEntities": 1, "minCommands": 4, "minTests": 3}
}
```

Every command cites `R1` because the user's one drawing motivates the
whole CRUD surface. If there were more regions you would distribute them.

### Golden 2 — picking an app-shape

A user wrote "I want a place to keep markdown notes for my research". The
canvas is empty (no regions). A good `templates` + `backend` section:

```json
{
  "templates": {
    "useTemplates": ["sqlite"],
    "declined": [
      {"name": "openrouter", "reason": "no LLM surface"},
      {"name": "google_oauth", "reason": "no auth, single-user local app"}
    ],
    "useAppShape": "markdown_editor",
    "backendBindings": {"markdown_editor": "sqlite"}
  },
  "backend": {
    "entities": [
      {
        "name": "Document",
        "fields": [
          {"name": "id",        "ty": "String", "required": true},
          {"name": "title",     "ty": "String", "required": true},
          {"name": "body",      "ty": "String", "required": true},
          {"name": "updatedAt", "ty": "u64",    "required": true},
          {"name": "tags",      "ty": "Vec<String>", "required": false}
        ],
        "motivatedByRegions": []
      }
    ],
    "commands": [
      {"name": "list_documents",   "input": "()",                  "output": "Vec<Document>", "motivatedByRegions": [], "summary": "Return all documents, newest first."},
      {"name": "load_document",    "input": "EntityIdPayload",     "output": "Document",      "motivatedByRegions": [], "summary": "Fetch a single document by id."},
      {"name": "save_document",    "input": "SaveDocumentPayload", "output": "Document",      "motivatedByRegions": [], "summary": "Create or update a document; returns the persisted record."},
      {"name": "delete_document",  "input": "EntityIdPayload",     "output": "bool",          "motivatedByRegions": [], "summary": "Delete a document; true if it existed."}
    ],
    "tests": [
      {"name": "json_document_store_round_trips", "module": "app_shapes::markdown_editor::tests", "covers": "save+load preserves Document fields"},
      {"name": "list_returns_empty_when_dir_missing", "module": "app_shapes::markdown_editor::tests", "covers": "edge: empty workspace yields []"},
      {"name": "list_sorts_newest_first", "module": "app_shapes::markdown_editor::tests", "covers": "list ordering is updatedAt desc"}
    ],
    "storage": {"kind": "sqlite", "path": "markdown_editor"}
  }
}
```

`useAppShape` + `backendBindings` lets BackendImpl drop in the scaffold's
`host.rs` (and the sqlite seam, when the binding is set) instead of
hand-rolling Document persistence. The `tags` field is the iteration's
contribution — a `// FILL: domain-specific document fields` marker the
implementer fills based on the user's "research notes" hint. Tests reuse
the scaffold's existing test names so the validator's grep on
`fn <name>` still passes after the copy.

## When finished

1. Save the plan to `{{PLAN_PATH}}` (your Edits already persist it).
2. Reply in chat with ONLY the sentence `backend_plan done` — no commentary,
   no summary. The validator reads the plan file, not your reply.
