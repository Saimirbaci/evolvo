# Stage prompts

Each file in this directory is the prompt template for one stage of the
multi-stage NewApp pipeline. The runner picks the file whose basename
matches `StageKind::slug()` and substitutes the following variables before
dispatching the Claude session:

- `{{APP_NAME}}` — `plan.app.name` (seeded from the feedback text; the
  BackendPlan stage is expected to rewrite it deliberately).
- `{{JOB_ID}}` — lineage job id.
- `{{PORT}}` — dev-server port this job's app should bind to (the
  runner has already rewritten the worktree's config to this port).
- `{{ROUTE}}` — the page route the canvas was drawn on top of.
- `{{PLAN_PATH}}` — absolute path to `plan.json` (always read and mutate
  this file; never hold a mental copy).
- `{{CANVAS_PNG}}` — absolute path to the canvas screenshot. Every stage
  that touches a user-visible artifact MUST `Read` this file directly — no
  prose summary of what's on it is acceptable.
- `{{ANNOTATIONS_PATH}}` — absolute path to the raw annotations JSON (may
  be the string `<none>` if the user submitted text-only feedback).
- `{{USER_TEXT}}` — feedback text, pre-wrapped in a `<user-content>` fence
  (see `_contract.md`'s prompt-injection defense; may be the string
  `<none>` if empty). Any `</user-content>` the user typed is neutralised
  before fencing so it cannot terminate the fence early.
- `{{VOICE_TRANSCRIPT}}` — voice transcript, fenced the same way (may be
  `<none>`).
- `{{REGION_INDEX}}` — human-readable dump of `plan.canvas.regions`, one
  line per region with its id / bbox / dominant color / labels. The
  `labels` portion (verbatim text the user typed on the canvas) is
  individually fenced per region; bbox/strokes/color are pipeline-computed
  geometry and are not fenced.
- `{{WORKTREE}}` — absolute path to the job's worktree.

## The golden rule every stage shares

The `plan.json` file at `{{PLAN_PATH}}` is the only durable handoff between
stages. Do not copy its contents into a chat message, a commit, or another
file. Write your output there, and nowhere else, by using Read + Edit /
Write on that path. When a later stage reads the plan, it sees what you
wrote — if you only "described" it, they see nothing.

Never paraphrase the canvas. If your output mentions the drawing, cite the
region ids (`R1`, `R2`, …) from `{{REGION_INDEX}}` and re-read
`{{CANVAS_PNG}}` in this session.

## Blocks the runner appends to every stage prompt

Beyond the template file, `build_stage_prompt` (runner.rs) appends:

- `_self_validate.md` — instructions for running the stage's deterministic
  validator from inside the session via `$EVOLVO_VALIDATE_BIN validate
  --stage <slug> ...`. Substitutes `{{STAGE_SLUG}}`, `{{JOB_DIR}}`,
  `{{WORKTREE}}`.
- **On Reflexion retries only**: a "Previous attempt failed validation"
  section carrying the failed checks from the prior attempt's
  `StageReport`. A stage gets up to `MAX_STAGE_ATTEMPTS` (currently 3)
  dispatches before the pipeline gives up; retries run against the
  worktree state the failed attempt left behind, and each retry's
  transcript lands in `logs/<slug>.attempt<N>.log`.
