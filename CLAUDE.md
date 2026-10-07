# Evolvo — Repository Guide for Claude

Evolvo is a **Tauri 2** desktop app with a **Leptos 0.8 (CSR / WASM)** frontend. It is a "white canvas" app: users draw / paste / record feedback on a blank canvas, submit it, and a **local lineage pipeline** turns each feedback row into a lineage job that a reviewer can approve/reject. Everything is stored as JSON files on the local filesystem — there is no database, no server, no cloud.

## Stack snapshot

| Layer        | Tech                                         | Entry                                  |
|--------------|----------------------------------------------|----------------------------------------|
| Desktop host | Tauri 2 (Rust)                               | `app/src-tauri/src/main.rs`            |
| Frontend     | Leptos 0.8 CSR + wasm-bindgen + web-sys      | `app/ui/src/main.rs`                   |
| Build (UI)   | Trunk → `app/ui/dist/`                       | `app/ui/Trunk.toml`                    |
| Build (host) | Cargo workspace (`app/src-tauri`, `app/ui`)  | `Cargo.toml` (workspace root)          |
| Storage      | Plain JSON files under `~/.evolvo/evolvo_workspace/` | `app/src-tauri/src/store.rs`     |

`EVOLVO_WORKSPACE_ROOT` env var overrides the default workspace root — use this in tests and local scripting.

## Workspace layout on disk

```
~/.evolvo/evolvo_workspace/
├── feedback/           # {id}.json — FeedbackRecord
├── lineage_jobs/       # {id}.json — LineageJobRecord
├── designs/            # {id}.json — DesignRecord (saved canvas designs; `draft.json` = autosave)
│   └── {id}.assets/    # content-addressed images (<sha256-prefix>.png) for that design
└── attachments/{feedback_id}/
    ├── canvas.png      # optional canvas screenshot
    ├── screen-NN.png   # ordered NewApp storyboard screens
    ├── screen-NN-paste-N.png # images pasted into one storyboard screen
    ├── paste-N.png     # pasted images
    └── voice.{ext}     # voice capture (webm/ogg/m4a/wav)
```

## Commands you will actually run

```bash
# Whole workspace typecheck (Rust + WASM crates)
cargo check --workspace

# Host-side unit tests (store, lineage state machine, commands)
cargo test -p evolvo_desktop

# UI crate typechecks (Leptos)
cargo check -p evolvo_ui --target wasm32-unknown-unknown

# Clippy (deny warnings on host code)
cargo clippy -p evolvo_desktop -- -D warnings

# Dev run (Tauri spawns Trunk via beforeDevCommand)
cargo tauri dev          # from app/src-tauri
# or from repo root:
cd app/src-tauri && cargo tauri dev

# Trunk only (UI hot-reload, no desktop shell)
cd app/ui && trunk serve   # uses scripts/trunk-dev.sh indirectly via Tauri
```

The Tauri config wires the UI build/serve at `app/ui/scripts/trunk-{dev,build}.sh` — do not duplicate that in docs; treat those scripts as authoritative.

## Core domain model

Defined in `app/src-tauri/src/types.rs`. All wire types are `serde(rename_all = "camelCase")` — the Leptos side sees camelCase JSON, the Rust side owns snake_case.

- `FeedbackRecord` — one user submission. Holds the legacy single canvas fields plus an ordered `screens` storyboard for multi-screen NewApp requests, pasted images, voice file, annotations (arbitrary JSON), window size, and status.
- `FeedbackStatus` — `new → triaged → in_lineage → resolved | rejected`.
- `LineageJobRecord` — created automatically when feedback is submitted. Status machine in `lineage.rs`.
- `LineageJobStatus` — `pending → triaging → planned → implementing → build_ready → merging → promoted | rejected | failed`. `can_approve()` gates UI action.
- `SubmitFeedbackPayload` — the Tauri command input; carries base64 for all binary attachments.

## Tauri commands (all invoked from the UI via `interop.rs`)

See `app/src-tauri/src/commands.rs` and `lib.rs::run()` / `main.rs` for registration.

- `app_health` → `AppHealth`
- `submit_feedback(SubmitFeedbackPayload)` → `FeedbackRecord` (also enqueues a lineage job)
- `list_feedback` / `load_feedback` / `delete_feedback`
- `list_lineage_jobs` / `load_lineage_job`
- `approve_lineage_job` / `reject_lineage_job` / `append_lineage_note`
- `open_workspace_path`
- `save_design` / `save_design_draft` / `list_designs` / `load_design` / `load_design_draft` / `delete_design` / `clear_design_draft` — saved canvas designs (logic in `designs.rs`, UI in `ui/src/design_bar.rs`). A design captures every storyboard screen (vector annotations + pasted images), the description and the feedback type; `load_design` returns screens in the same `EditableFeedbackScreen` shape as a feedback revision. Ids are host-minted `design-<ms>` plus the reserved `draft` (validated by `store::is_valid_design_id` before any path is built). Images are content-addressed and written before the record, unreferenced ones are collected after — a crash never leaves a record pointing at a missing image. The panel autosaves the draft (1.5s debounce) and restores it on the next launch if the panel is blank; it only clears the draft when the startup load succeeded.
- `generate_sketch(SketchRequest)` → `SketchResponse` — **experimental** AI sketching (`sketch.rs` host / `ui/src/sketch.rs`). Runs `claude -p --json-schema … --tools ""` (no tools) and returns add/update/delete ops in the canvas annotation JSON; the UI sanitises them and applies them as one undo step. Opus for empty-canvas generation, Sonnet for edits; override with `EVOLVO_SKETCH_MODEL_GENERATE` / `EVOLVO_SKETCH_MODEL_EDIT`.

- `take_desktop_capture` / `close_desktop_overlay` / `open_desktop_revision` / `capture_desktop_window` — **desktop overlay** ("evolve any app", `desktop.rs` host / `ui/src/desktop_overlay.rs`). A global shortcut (default `CommandOrControl+Shift+E`, override `EVOLVO_DESKTOP_SHORTCUT`) records the frontmost app (name, window title, pid, executable, bundle id), freezes its monitor (≤2560px), and shows a borderless always-on-top `desktop-overlay` window (UI mounts `DesktopOverlayApp` when `window.__EVOLVO_MODE__ == 'desktop'`; the panel remounts per capture). Submission is the normal `submit_feedback` with `targetApp` + `backgroundBase64` (clean frame → `frame.png`, so "Revise on Canvas" reopens the overlay over the original screen via `open_desktop_revision`). Inside the overlay, **W** opens a window picker (frozen screen dimmed, hover highlights the frontmost window under the cursor from `DesktopSession.windows`, Enter takes the pre-highlighted active window, Esc returns) and `capture_desktop_window` captures that one window on its own; **S** returns to the whole screen. Switching modes clears the drawing. In window mode the frame is shown centred (`.desktop-frame-window`) and `feedback_panel::composite_page_and_annotations` maps strokes through the displayed `.desktop-frame` rect, so the submitted image is the window only. The in-app white canvas / Shell FAB is unchanged — this is a separate surface.
- `resolve_target_repo` — finds the external app's source (see **External apps** below); optional user path/URL override from the Lineage card.
Every new command MUST be registered in the `invoke_handler` in `src-tauri/src/lib.rs` AND mirrored in `app/ui/src/interop.rs`.

## Frontend structure (Leptos 0.8)

```
app/ui/src/
├── main.rs            # mount_to_body
├── app.rs             # NewApp Home content mounted inside <Shell/> — iterations rewrite this
├── shell.rs           # INVARIANT chrome: app bar, Lineage page, Star Us, Feedback FAB, Canvas overlay
├── canvas.rs          # the drawing / annotation canvas (large — ~1k lines)
├── feedback_panel.rs  # submission form + attachments
├── toolbar.rs         # canvas tools
├── voice.rs           # MediaRecorder wrapper
├── interop.rs         # invoke() wrappers (Tauri <-> WASM bridge)
└── types.rs           # UI mirrors of wire types
```

`interop.rs` is the ONLY place that should call `window.__TAURI__.core.invoke`. Every new Tauri command gets a typed wrapper here.

## Conventions

- **No unwrap in production paths.** Tests may use `.unwrap()`; commands return `Result<T, String>`.
- **Serde `camelCase` for wire types**, snake_case for internal. The `tolerates_extra_fields` test pattern is load-bearing — keep new types forward-compatible by default (avoid `deny_unknown_fields` unless the type is a security boundary).
- **Path sanitisation**: `store::sanitise_filename` strips anything non-`[A-Za-z0-9._-]`. Use it for ANY filename derived from user/UI input. Never build a `PathBuf` by concatenating user strings directly.
- **IDs**: feedback IDs look like `fb-<unix_ms>`; lineage job IDs are generated by `lineage.rs`. Do not invent new ID schemes without updating both.
- **Attachments**: always routed through `Store::save_attachment` / `read_attachment`. They enforce the per-feedback directory scope (`attachments/{feedback_id}/...`).
- **Tests live next to code** (`#[cfg(test)] mod tests`). Prefer `tempfile::tempdir()` for any filesystem-touching test so they remain hermetic.
- **Release profile** (`opt-level = "z"`, `lto = true`, `strip = true`) is tuned for WASM size. Do not relax it casually — measure `dist/` size before/after.

## Gotchas

- Tauri 2 invoke handler registration is mandatory; a command defined with `#[tauri::command]` but not listed in `invoke_handler!` will fail silently at call time with `command X not found`.
- Trunk does NOT type-check the workspace automatically. Run `cargo check --workspace` before declaring UI work done — a Leptos view macro will happily compile nonsense into a runtime panic.
- `withGlobalTauri: true` is set — `interop.rs` relies on `window.__TAURI__`. Don't switch to module import without updating both sides.
- Canvas pastes/screenshots go through the clipboard + canvas→PNG path; the base64 encode happens in WASM before `submit_feedback`. Large images will dominate the IPC payload — keep attachments sane (soft-cap at a few MB).
- NewApp feedback can contain up to eight ordered storyboard screens. Each keeps independent vectors, pasted images, name, intended route, and description; submission captures the underlying route once, composites every screen separately, and stages a `storyboard.json` manifest so every agent stage reads the complete flow.
- `.evolvo/evolvo_workspace/` is outside the repo. Use `EVOLVO_WORKSPACE_ROOT` to point at a temp dir for reproducible runs.

## Product invariants (read this first)

See `.claude/rules/common/product-invariants.md` for authoritative text. In short:

- **Lineage always stays.** The feedback → lineage-job pipeline is permanent.
- **Feedback Overlay always stays.** Reachable from every screen, every mode.
- **The Canvas is a per-page overlay, not a tab.** The canvas module may be rewritten or replaced, but the resulting app must let the user open the Canvas overlay *on top of every page / route* to annotate the actual screen they have feedback about. A dedicated "canvas tab" where the canvas only exists as its own screen violates this invariant.
- **One trigger opens BOTH Canvas overlay and Feedback panel.** Every Iteration ships a single Feedback FAB bound to a single `panel_open` signal — clicking it brings up the drawing surface *and* the submission panel together. Iterations must keep this: exactly one affordance, one signal, both surfaces visible together. Never a separate "draw" button and "send feedback" button; never leave a stale prior trigger behind after redesign; the button must be clearly labelled (`aria-label` + visible `title`) so the user knows what it does.
- **Lineagees are saveable and forkable into standalone apps.** Lineage state is a portable, self-contained artifact that can be renamed / cloned into a new Evolvo-shaped app with its own identity.

These outrank refactor aesthetics and most feature requests. Changes that violate them are product decisions — escalate.

## `shell.rs` is invariant; `app.rs` is where the NewApp lives

The UI is split into two layers:

- **`app/ui/src/shell.rs`** — the permanent Evolvo chrome. Owns the app bar, the Lineage navigation + page, the "Star Us" link, the single Feedback FAB, and the Canvas overlay + feedback panel composition. **Every iteration keeps mounting `<Shell>`.** The shell is what enforces I-P1 through I-P3b (Lineage always reachable, Feedback Overlay always reachable, Canvas overlay on every page, one trigger for both surfaces). Do not delete, duplicate, or re-implement this chrome inside `app.rs` — if a chrome change is needed, edit `shell.rs` directly.
- **`app/ui/src/app.rs`** — the **NewApp content area**. When the user asks for a NewApp, this is the file that gets rewritten: replace `HomePage` with the new app's root component / router / layout and add further modules alongside it. Keep `App` mounting `<Shell>` with the new content as its children. Because the shell wraps the content, every page/route of the NewApp is automatically annotatable — the Canvas overlay mounts on top of whatever `app.rs` renders when the user clicks the FAB.

Shell exposes `PanelOpen` via `provide_context` so NewApp content can read the Canvas/Feedback open state (e.g. to hide copy from the submission screenshot) without re-implementing the FAB.

## Stylesheet layers — tokens / shell / base / app

Trunk loads four stylesheets in cascade order (`app/ui/index.html`):

1. **`app/ui/styles/tokens.css`** (protected) — the design vocabulary: every CSS custom property (`--bg`, `--accent`, `--space-*`, `--text-*`, `--shadow-*`, …). Iterations may change token *values* for a new visual identity but must never delete or rename a token — `validate_frontend_impl` / `validate_final` grep for the full name list.
2. **`app/ui/styles.css`** (protected shell layer) — app bar, canvas overlay, feedback panel, toolbar, lineage pages, evolve modal, feedback FAB. Do not add NewApp-specific rules here.
3. **`app/ui/styles/base.css`** (protected, append-only) — reusable component primitives built only from tokens: `.btn`/`.btn-primary`/`.btn-danger`/`.btn-ghost`, `.card`, `.input`, `.field`, `.empty-state`, `.badge`. Extending with new primitives is fine; deleting or modifying an existing one is not.
4. **`app/ui/styles/app.css`** (iteration-owned) — the only stylesheet the FrontendImpl stage may edit. Must use `var(--...)` tokens for colors/font-size/box-shadow — no raw hex/rgb/hsl. The growth gate (`STYLES_MIN_GROWTH_BYTES`, `app/src-tauri/src/validators.rs`) and the design lint both target this file; legacy worktrees without `styles/app.css` fall back to treating `app/ui/styles.css` as the growth target.

See `prompts/stages/frontend_impl.md` for the exact lint rules an agent implementing a NewApp must satisfy.

## Canvas + Feedback overlay rules (load-bearing, easy to regress)

These concrete rules are what make I-P2 / I-P3 actually work. Previous
iterations regressed them — read before touching `app/ui/src/canvas.rs`,
`app/ui/src/app.rs`, or the overlay CSS in `app/ui/styles.css`.

1. **One FAB, opens both.** The ✎ FAB toggles *both* `canvas_open` and
   `feedback_open` in lockstep. Do not reintroduce a second ✦ FAB — the
   product wants a single affordance. The feedback panel is still always
   reachable (I-P2 holds via this single button).

2. **The underlying page must remain visible while the canvas is open.**
   The whole point of the canvas is annotating the *current route*. That
   means:
   - `.canvas-overlay` uses `background: transparent` (never a white/opaque
     backdrop, never `backdrop-filter: blur`).
   - The `<canvas>` bitmap is cleared with `ctx.clear_rect(...)` on every
     render — **never** `fill_rect` with white. A white fill on the bitmap
     defeats the transparent CSS and hides the page.
   - `.canvas-surface` / `.stage` inside the overlay have
     `background: transparent`.

3. **`.stage` needs a flex parent.** `.stage` is `flex: 1`. `.canvas-overlay`
   must be `display: flex`, or the stage collapses to 0×0 and the canvas
   silently swallows no pointer events (looks like "drawing is broken").

4. **Pointer-event layering.** `.canvas-overlay { pointer-events: none }`
   and `.canvas-overlay > * { pointer-events: auto }` — so clicks outside
   the drawing surface still reach the toolbar / close button, but the
   overlay itself does not trap events over empty space. (If you ever want
   clicks outside the canvas to fall through to the page underneath, that
   is the hook.)

5. **Z-index contract.** Canvas overlay is `z-index: 50`. The Feedback
   `.panel` must be **above** it (currently `z-index: 55`) so the panel is
   visible and interactable while the canvas overlay is open. If you add
   new floating UI, respect: overlay 50 < panel 55 < FAB stack (45 is fine
   because FAB lives outside the overlay).

6. **Live canvas bitmap stays transparent; the *submitted* screenshot is a
   composite.** The on-screen canvas is `clear_rect`-ed so the overlay is
   see-through (do not "fix" this by filling white). At submit time,
   `feedback_panel.rs` hides `.canvas-overlay` and `.panel`, calls the
   `capture_window_png` Tauri command (xcap-backed) to grab the real page,
   then draws the annotation PNG on top of that page image in an offscreen
   canvas and submits the composite. The agent reviewing feedback therefore
   gets a single PNG with page context + user strokes. If you rewire the
   submit flow, preserve the hide-capture-composite step — without it,
   Claude sees only disembodied marks on white.

## Dev-server port hygiene

The runner rewrites this iteration to port `1530 + N`. If `cargo tauri dev`
shows a blank WebView with a console error like `Failed to load resource:
Could not connect to the server` at `127.0.0.1:<port>`, a stale `trunk`
process from a previous iteration is almost certainly holding the port and
your new `trunk` silently failed to bind. Check:

```bash
lsof -iTCP -sTCP:LISTEN -P -n | grep trunk
```

Kill the stale PID and restart `cargo tauri dev`. Do **not** work around
this by rewriting the port in config — the iteration port is a contract
with the runner.

## Verify-before-done — actually run the app

Type-checks and unit tests do not prove the feature works. Before claiming a change is complete:

1. Run `cargo check --workspace` and `cargo test -p evolvo_desktop`. Both must pass.
2. **Start the app** with `cargo tauri dev` (or `bash scripts/run-iteration.sh` inside a lineage worktree). Wait for Trunk to print `server listening at http://127.0.0.1:<port>`.
3. **Exercise the change** in the running app: the affected route, the golden path, and 1-2 edge cases adjacent to what was asked. Confirm none of the four invariants regressed.
4. Only then commit.

If you genuinely cannot run the app in the current environment, say so plainly in your summary — don't claim success you didn't observe.

## Which agent ran a lineage job
A lineage job persists its selected agent as `agent` on the `LineageJobRecord` (`lineage_jobs/<job-id>.json`) and also in `run.json` under the job workspace. Supported backends and their transcripts:

- **Claude Code** (`AgentKind::ClaudeCode`) — binary `claude`, transcript `claude.log` (stream-json / JSONL).
- **Codex CLI** (`AgentKind::CodexCli`) — binary `codex`, transcript `codex.log` (`codex exec --json` JSONL with its own event schema).
- **Gemini CLI** (`AgentKind::GeminiCli`) — binary `gemini`, transcript `gemini.log` (free-form text, no stable JSONL).
- **Forge** (`AgentKind::Forge`) — binary `forge` (forgecode.dev / tailcallhq), transcript `forge.log`. One-shot mode: `forge -p <prompt>`. Provider/model selection lives in `forge.yaml` at the worktree root or via `forge provider login` once per host. The wire-type variant carries `#[serde(alias = "open_code")]`, so lineage jobs persisted by older builds (which previously slotted OpenCode here) deserialise into the Forge variant — those records render as Forge in the UI even though they were originally executed by `opencode`.

Project-guide discovery differs per CLI; the runner materialises symlinks at the worktree root so all four agents pick up `CLAUDE.md`:
- Claude Code reads `CLAUDE.md` and `.claude/agents/*` natively.
- Codex / Forge see `AGENTS.md` (symlink to `CLAUDE.md`).
- Gemini sees `GEMINI.md` (symlink to `CLAUDE.md`).

The multi-stage NewApp pipeline works for every backend because it drives off `plan.json`, not the agent log (see `app/src-tauri/src/stages.rs`). Retry/Resume reuse the job's persisted `agent`, so a failed Codex run retries as Codex, not Claude.

The feedback panel's Agent chip row probes `list_available_agents` on mount and greys out chips whose binary is not on PATH — install the missing CLI and restart Evolvo to re-probe (availability is cached per-process).

## Stage self-correction: Reflexion retries, validate CLI, checkpoints

Three load-bearing mechanisms in the stage pipeline (`stages.rs` / `runner.rs`):

- **Reflexion retries.** A validator rejection no longer fails the job outright — `run_stage` re-dispatches the stage up to `MAX_STAGE_ATTEMPTS` (3) times, rendering the previous attempt's failed checks into the prompt (`build_stage_prompt`). Retries keep the worktree as the failed attempt left it; each retry's transcript is `logs/<slug>.attempt<N>.log`. A `plan.json` the agent corrupted counts as a failed attempt (parse error fed back), not a hard error.
- **`evolvo_desktop validate` CLI.** The exact stage validators are callable headlessly: `evolvo_desktop validate --stage <slug> --job-dir <dir> --worktree <dir> [--json]` (exit 0 = accepted, 1 = rejected with `FAIL` lines, 2 = usage error). The runner passes the host binary to every stage session as `$EVOLVO_VALIDATE_BIN`, and `prompts/stages/_self_validate.md` instructs agents to iterate until it passes *before* exiting. Agents must never edit `validators.rs` to make it pass — the pipeline runs the host's copy.
- **Green-stage checkpoints.** After a stage validates green, the runner best-effort commits the worktree as `stage(<slug>): green checkpoint — job <id>` (identity `Evolvo Pipeline <pipeline@evolvo.local>`). Plan-only stages produce no commit (clean tree). These anchors exist so recovery can reset to the last green stage instead of discarding the worktree.
- **Plan-review gate (opt-in).** The feedback panel's "Review plan first" checkbox sets `pause_after_plan` on the job: the pipeline stops after BackendPlan validates, flips the job to `Planned`, and waits. The reviewer reads `plan.json` — including the planner's `openQuestions` (recorded guesses on ambiguities) — then Approve resumes implementation (`plan_approved_at_unix_ms` is stamped so the pause never re-fires) or Reject kills it. Legacy `Planned` jobs without the flag keep historical approve semantics. At the pause the reviewer can type answers under each open question: **Approve plan** records them (`answer_plan_questions` with `replan:false`) and starts implementation; **Answer & re-plan** (`replan:true`) stores them in `plan.answeredQuestions`, rewinds `plan.stage` to `Seeded` so BackendPlan re-dispatches, and the re-plan continues straight into implementation when every open question is answered and nothing else needs a decision (`plan::plan_review_settled_by_answers`) — otherwise it pauses again. Answers are binding for every stage (see `prompts/_contract.md`).
- **Behavior-first backends (anti-mockup).** `plan.backend.behaviors[]` records what the app *does* — each entry has a `trigger` (`userAction`/`timer`/`backgroundWorker`/`externalEvent`), an `effect`, `changesState`, an `engine` (`deterministic`/`llm`), the host fn it's `implementedIn`, and anchors. `validate_backend_plan` fails a plan with no behaviors unless `backend.staticApp.reason` justifies it, and fails AI-shaped requests (agents/assistants/AI/LLM in the user's words, canvas labels or app identity — `plan::ai_signals`) that have no `llm` behavior or no LLM template (`claude_cli` preferred, `openrouter`). `validate_backend_impl` and `validate_final` check every behavior fn exists, contains an anchor and is reachable (registered or referenced outside its definition); `validate_e2e_plan` requires every behavior to be named in some scenario's `verifiesBehaviors`. A plan marked static, with non-empty `backend.simulatedBehaviors`, or whose `openQuestions` use narrow simulation wording ("simulated", "bookkeeping only", "mocked", "no real model" — see `plan::SIMULATION_PHRASES`; generic words like "placeholder"/"stub"/"hardcoded" deliberately don't count) **auto-pauses** for plan review even when the job didn't request it (`plan::plan_review_required_reason`). `templates/integrations/claude_cli/` is the no-API-key model engine for generated apps (`LlmEngine` trait, `ClaudeCli`, `ScriptedEngine` for tests).
- **Runtime hardening.** Every agent session and validator tool runs in its own process group with a hard timeout (`EVOLVO_AGENT_TIMEOUT_SECS`, default 3600s; validators 15 min) — a hung CLI gets its whole group killed instead of wedging the job. Concurrent agent sessions across all jobs are capped by a process-wide semaphore (`EVOLVO_MAX_CONCURRENT_JOBS`, see `concurrency.rs`) so a burst of Advances can't fork unbounded CLI processes. Job-record mutations are serialised by a per-job lock (`lineage::job_write_lock`) and written atomically (tmp+rename, shared via `fsutil::write_atomic`). Failure-path status writes (background threads flipping a job to `Failed`/`BuildReady` after an error) go through `lineage::force_status_with_retry`, which retries a few times with backoff and logs loudly on exhaustion — a job must never silently stay `Implementing`/`Merging` because the failure-path write itself failed. On startup, `runner::reconcile_orphaned_jobs` fails any job left `Implementing` by a crashed host so Resume/Retry become available; Resume refuses jobs whose run is still live.
- **Soft Retry.** When the worktree contains a `green checkpoint` commit and `plan.json` exists, Retry does `git reset --hard` to the last checkpoint + `git clean -fd` and resumes the pipeline — green stages keep their work, only the failed stage re-runs. The destructive teardown (fresh worktree from source HEAD) remains the fallback for legacy runs or pipelines with no checkpoint. The soft-retry decision (checkpoint lookup + worktree reset/teardown) is serialised per job id via `lineage::job_retry_lock` so two concurrent retry calls can't interleave their worktree mutations; `runner::last_green_checkpoint` / `reset_worktree_to` also fail gracefully (not panic) if the worktree vanished between a caller's existence check and the call.
- **Failure legibility.** Validator tool gates capture the build tool's output and put the error tail in the failed check's detail (so the Reflexion retry prompt sees the actual compiler error, not "exited non-zero") — the detail cap is 2000 chars (`validators::MAX_DETAIL_CHARS`), generous enough for a real compiler error tail to survive into the retry prompt. `validate_final` also greps the product invariants deterministically (shell.rs keeps `FeedbackFab`+`panel_open`, app.rs mounts `<Shell>`, styles keep `.canvas-overlay`) and `// FILL:` markers count as stub smell. The Lineage detail panel renders the planner's `openQuestions` as a highlighted checklist while a job waits in `Planned`. A stage's `plan.json` write on the green path is itself gated: `stages::persist_stage_outcome` demotes an otherwise-passing stage to `Failed` (with the reload/write error in the detail) if `plan.json` can't be reloaded and re-persisted with the advanced stage marker — a stage is never reported `Green` while `plan.stage` silently fails to advance.
- **Workspace retention + corrupted-entity quarantine.** On startup (after `reconcile_orphaned_jobs`, on a background thread) `runner::cleanup_old_workspaces` reclaims `lineage_workspaces/<job-id>/` directories for jobs in a terminal `Failed`/`Rejected` state whose `updated_at_unix_ms` is older than `EVOLVO_WORKSPACE_RETENTION_DAYS` (default 30; `0` disables cleanup). `Promoted`/`BuildReady`/non-terminal jobs are never touched. A workspace directory with no matching job record is quarantined into `lineage_workspaces/.orphaned/` rather than deleted — the record may have been lost, not the job. Separately, `store::list_json_entities` quarantines any `feedback/*.json` or `lineage_jobs/*.json` file that fails to parse into `<dir>/.corrupted/` (instead of silently skipping it with an `eprintln!`); the total quarantined count is surfaced as `AppHealth.corrupted_entity_count`. `Store::init_workspace` also sweeps leftover `*.tmp` debris from a `write_atomic` that crashed mid-write, and creates workspace directories with owner-only permissions (`0700` on Unix, via `fsutil::create_dir_all_private`).

## External apps — evolving another application's repo

Feedback with a `targetApp` (desktop overlay) runs against **that app's repository**, not Evolvo (`external.rs`):

- **Repo resolution** (`external::resolve_repo`, background thread via `start_resolution`, progress as `source repo:` job notes, result on `targetApp.repoPath/repoUrl/repoSource/repoStatus/repoError`): user hint (local path or https URL) → repo next to the executable → remembered mapping (`<workspace>/external_apps.json`, keyed by bundle id) → git checkout under `~/{Dev,code,src,Projects,GitHub,repos,…}` whose dir name matches the app → upstream lookup (`claude -p` with WebSearch, GitHub search fallback; URL must be `https://{github,gitlab,codeberg,bitbucket}/owner/repo`, verified with `git ls-remote`) and `git clone --filter=blob:none` into `<workspace>/external_repos/<owner>__<repo>`. Submitting starts resolution immediately; Evolve/auto-evolve on an unresolved job resolves first and then starts the agent (`commands::run_after_resolution`).
- **Run:** `runner::prepare_run` forks the worktree from the target repo, skips CLAUDE.md aliases and port rewrites, and uses `prompts/external_app.md`. Always a single agent session (never the NewApp multi-stage pipeline). After the agent exits, `external::validate_external_gates` runs detected checks (`cargo check [--workspace]`, `<npm|pnpm|yarn|bun> run build`, `go build ./...`; none detected → pass) with `EVOLVO_EXTERNAL_GATE_TIMEOUT_SECS` (default 3600).
- **Promotion** merges into the target repo with the same safety rules; `promotion::promote_job` swaps in the external gates. Retry tears down the worktree against `job.source_repo`.
- **Portability (I-P4):** bundle export strips `targetApp.repoPath`/`pid`/status; `repoUrl`, bundle id and name stay so an import can re-resolve.

## Promotion — merging a lineage branch back into the source repo

`BuildReady → Merging → Promoted` is a real merge, not a status flip. When a reviewer clicks Approve on a `BuildReady` job, `commands::approve_lineage_job` flips the job to `Merging` synchronously and hands off to `promotion::start_promotion`, which runs on a background thread under two locks: the per-job write lock (same as every other job mutation) and a process-global `concurrency::PromotionSlot` — only one promotion runs at a time host-wide, since two simultaneous merges into the same source repo is never safe. A second concurrent Approve is refused with a note ("another promotion is already in progress") and the job bounces back to `BuildReady`.

`promotion::promote_job` (`promotion.rs`) is the safety-critical core:

- **Branch resolution.** `runner::prepare_run` snapshots the source repo's checked-out branch (`runner::checked_out_branch`) at worktree-creation time and persists it as `LineageJobRecord.base_branch`. Promotion refuses if the source repo's currently checked-out branch differs from that snapshot (the reviewer switched branches since Advance) or if `base_branch` was never captured (detached HEAD at start, or a legacy job).
- **Preconditions (checked before anything touches the source repo):** working tree must be clean of tracked-file changes (`git status --porcelain=v1 --untracked-files=no` — untracked files are deliberately NOT a blocker, since neither `git merge` nor `git reset --hard` touches them); the lineage branch must exist; the worktree's HEAD must match the branch tip; the source repo's checked-out branch must match `base_branch`. Any failure appends a note and returns the job to `BuildReady` without mutating the repo.
- **Merge.** Snapshots pre-merge HEAD, prefers `git merge --ff-only`, falls back to `git merge --no-ff` with a commit authored as `Evolvo Pipeline <pipeline@evolvo.local>` (same identity as checkpoint commits) and message `promote(lineage): <job-id> — <summary>`. A conflict runs `git merge --abort` and refuses with the conflicting paths in the note — the source repo is left exactly as it was.
- **Post-merge gates.** `cargo check --workspace` and `cargo test -p evolvo_desktop` run against the source repo's new HEAD. Either failing does `git reset --hard <pre-merge SHA>` (safe — the precondition guarantees a clean tree) and returns the job to `BuildReady` with the gate's output tail in the note. Production gates are supplied by `default_gates()`; tests inject stub `Gate` closures via `promote_job_with_gates` (the gate-stub seam) so the git merge/rollback logic can be exercised without paying for real `cargo` runs in a tempdir.
- **Success.** Advances `Merging → Promoted`, stamps `promotedAtUnixMs` + `promotedCommit` (the merge/ff SHA — a self-describing value, not a host-state pointer, per I-P4) on the job record, and appends a note with the SHA. The worktree and lineage branch are **not** deleted — they remain the reviewable artifact; cleanup is a future batch.
- **Crash recovery.** `runner::reconcile_orphaned_jobs` also handles jobs left in `Merging` by a crashed host: if `promoted_commit` is already stamped, or if the source repo's HEAD independently verifies as containing the lineage branch tip (`git merge-base --is-ancestor`), the job finalizes to `Promoted`; otherwise it returns to `BuildReady` so Approve can retry. See `runner::reconcile_orphaned_merge` for the exact decision logic.

`LineageEngine::transition` deliberately rejects `BuildReady → Promoted` as a direct `Transition::Approve` — that path only existed pre-promotion and would skip the merge entirely. `commands::approve_lineage_job` intercepts `BuildReady` before it ever reaches the engine's state machine.

## Debugging a lineage run — `claude.log` is JSONL

Each lineage job's `claude.log` (under `~/.evolvo/evolvo_workspace/lineage_workspaces/<job-id>/`) is captured with `claude -p ... --output-format stream-json --verbose`, so it is **one JSON event per line**, not human-readable prose. Each line has a `type` (`system`, `assistant`, `user`, `result`, etc.); assistant messages carry `message.content[]` with `tool_use` / `text` entries; user messages carry `tool_result` entries.

Useful one-liners (requires `jq`):

```bash
LOG=~/.evolvo/evolvo_workspace/lineage_workspaces/<job-id>/claude.log

# Every tool call the agent made (name + truncated input)
jq -c 'select(.type=="assistant") | .message.content[]?
       | select(.type=="tool_use") | {name, input: (.input | tostring[0:200])}' "$LOG"

# Did the agent actually Read the staged canvas.png?
jq -c 'select(.type=="assistant") | .message.content[]?
       | select(.type=="tool_use" and .name=="Read")
       | .input.file_path' "$LOG" | grep -i canvas.png

# Final assistant text
jq -r 'select(.type=="result") | .result // .message.content[-1].text // empty' "$LOG"
```

If `claude.log` is empty or missing tool_use events for the staged attachments, the model never read them — surface that in the lineage notes rather than silently shipping an iteration that ignored the user's drawing.

### Other agents
`codex.log` is also JSONL but with a different schema — each line has a `type` (`agent_message`, `tool_call`, `tool_result`, `task_complete`, etc.). Grep for `"type":"tool_call"` to see which tools fired and pipe through `jq` the same way.

`gemini.log` has no standard JSONL — fall back to `grep -n 'tool:'` and `grep -n 'error'` for a quick triage.

`forge.log` shape depends on the provider Forge is configured with; start by tailing the file and adapting the Claude/Codex recipes as needed.

## Lineage iteration port convention

Each lineage iteration runs on its own dev-server port so multiple iterations can run side-by-side without collisions.

- Host Evolvo (iteration 0): port **`1530`** — this is `BASE_DEV_PORT` in `app/src-tauri/src/runner.rs`.
- Iteration `N`: port **`1530 + N`**.

Before spawning the claude run in a lineage worktree, the runner rewrites `app/src-tauri/tauri.conf.json`, `app/ui/Trunk.toml`, and `app/ui/scripts/trunk-dev.sh` in that worktree to the iteration's port. The runner also sets `EVOLVO_ITERATION_PORT=<port>` in the environment of the iteration's Run command. If you rewrite the stack, honour that env var in your replacement startup script — never hardcode `1530`.

## After implementation — commit, then start the new version

When the work is done and verified:

1. `git add -A && git commit -m "<conventional commit>"` — include `CLAUDE.md`, rules, and agent updates in the same commit when they travel with a behaviour change.
2. Start the iteration's app (`cargo tauri dev` or the equivalent for the current stack) so the reviewer lands on a live build. Leave it running.
3. In your final summary note which port the iteration is serving on and how you verified it.
4. Change the LineageJob status to `build_ready` so the next iteration can start from it.

## Rules & agents

Project rules live under `.claude/rules/` — read them before editing:
- `.claude/rules/rust/` — Rust / Cargo conventions
- `.claude/rules/tauri/` — Tauri 2 command and config conventions
- `.claude/rules/frontend/` — Leptos 0.8 + Trunk + interop conventions
- `.claude/rules/common/` — commit / branching / review conventions

Agents:
- `staff-feedback` — works the local feedback queue (reads JSON from `~/.evolvo/evolvo_workspace/feedback/`) and ships fixes end-to-end.
- `staff-build-engineer` — keeps `cargo check`, `cargo test`, `trunk build`, `cargo tauri build` green; owns toolchain, CI hygiene, bundle size.
- `staff-architect-self-evolving-software` — designs the lineage → promotion pipeline so the app can safely absorb its own feedback.

# After each session:
- Commit the changes with a message describing the work done.
- Start the new version of the app (the iteration you just built) so the reviewer can see it live when they start their review.
- Update the LineageJob status to `build_ready` so the next iteration can start from it.
- In your final summary, note which port the iteration is serving on and how you verified it.
