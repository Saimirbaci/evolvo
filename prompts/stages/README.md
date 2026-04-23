# Stage prompts

Each file in this directory is the prompt template for one stage of the
multi-stage NewApp pipeline. The runner picks the file whose basename
matches `StageKind::slug()` and substitutes the following variables before
dispatching the Claude session:

- `{{APP_NAME}}` — `plan.app.name` (seeded from the feedback text; the
  BackendPlan stage is expected to rewrite it deliberately).
- `{{JOB_ID}}` — lineage job id.
- `{{ITERATION}}` — iteration number (1-based).
- `{{ROUTE}}` — the page route the canvas was drawn on top of.
- `{{PLAN_PATH}}` — absolute path to `plan.json` (always read and mutate
  this file; never hold a mental copy).
- `{{CANVAS_PNG}}` — absolute path to the canvas screenshot. Every stage
  that touches a user-visible artifact MUST `Read` this file directly — no
  prose summary of what's on it is acceptable.
- `{{ANNOTATIONS_PATH}}` — absolute path to the raw annotations JSON (may
  be the string `<none>` if the user submitted text-only feedback).
- `{{USER_TEXT}}` — verbatim feedback text.
- `{{VOICE_TRANSCRIPT}}` — verbatim voice transcript (may be `<none>`).
- `{{REGION_INDEX}}` — human-readable dump of `plan.canvas.regions`, one
  line per region with its id / bbox / dominant color / labels.
- `{{WORKTREE}}` — absolute path to the iteration worktree.

## The golden rule every stage shares

The `plan.json` file at `{{PLAN_PATH}}` is the only durable handoff between
stages. Do not copy its contents into a chat message, a commit, or another
file. Write your output there, and nowhere else, by using Read + Edit /
Write on that path. When a later stage reads the plan, it sees what you
wrote — if you only "described" it, they see nothing.

Never paraphrase the canvas. If your output mentions the drawing, cite the
region ids (`R1`, `R2`, …) from `{{REGION_INDEX}}` and re-read
`{{CANVAS_PNG}}` in this session.
