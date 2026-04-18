# NoIDE — Repository Guide for Claude

NoIDE is a **Tauri 2** desktop app with a **Leptos 0.8 (CSR / WASM)** frontend. It is a "white canvas" app: users draw / paste / record feedback on a blank canvas, submit it, and a **local sandbox pipeline** turns each feedback row into a sandbox job that a reviewer can approve/reject. Everything is stored as JSON files on the local filesystem — there is no database, no server, no cloud.

## Stack snapshot

| Layer        | Tech                                         | Entry                                  |
|--------------|----------------------------------------------|----------------------------------------|
| Desktop host | Tauri 2 (Rust)                               | `app/src-tauri/src/main.rs`            |
| Frontend     | Leptos 0.8 CSR + wasm-bindgen + web-sys      | `app/ui/src/main.rs`                   |
| Build (UI)   | Trunk → `app/ui/dist/`                       | `app/ui/Trunk.toml`                    |
| Build (host) | Cargo workspace (`app/src-tauri`, `app/ui`)  | `Cargo.toml` (workspace root)          |
| Storage      | Plain JSON files under `~/.noide/noide_workspace/` | `app/src-tauri/src/store.rs`     |

`NOIDE_WORKSPACE_ROOT` env var overrides the default workspace root — use this in tests and local scripting.

## Workspace layout on disk

```
~/.noide/noide_workspace/
├── feedback/           # {id}.json — FeedbackRecord
├── sandbox_jobs/       # {id}.json — SandboxJobRecord
└── attachments/{feedback_id}/
    ├── canvas.png      # optional canvas screenshot
    ├── paste-N.png     # pasted images
    └── voice.{ext}     # voice capture (webm/ogg/m4a/wav)
```

## Commands you will actually run

```bash
# Whole workspace typecheck (Rust + WASM crates)
cargo check --workspace

# Host-side unit tests (store, sandbox state machine, commands)
cargo test -p noide_desktop

# UI crate typechecks (Leptos)
cargo check -p noide_ui --target wasm32-unknown-unknown

# Clippy (deny warnings on host code)
cargo clippy -p noide_desktop -- -D warnings

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

- `FeedbackRecord` — one user submission. Holds canvas screenshot ref, pasted images, voice file, annotations (arbitrary JSON), window size, status.
- `FeedbackStatus` — `new → triaged → in_sandbox → resolved | rejected`.
- `SandboxJobRecord` — created automatically when feedback is submitted. Status machine in `sandbox.rs`.
- `SandboxJobStatus` — `pending → triaging → planned → implementing → build_ready → merging → promoted | rejected | failed`. `can_approve()` gates UI action.
- `SubmitFeedbackPayload` — the Tauri command input; carries base64 for all binary attachments.

## Tauri commands (all invoked from the UI via `interop.rs`)

See `app/src-tauri/src/commands.rs` and `lib.rs::run()` / `main.rs` for registration.

- `app_health` → `AppHealth`
- `submit_feedback(SubmitFeedbackPayload)` → `FeedbackRecord` (also enqueues a sandbox job)
- `list_feedback` / `load_feedback` / `delete_feedback`
- `list_sandbox_jobs` / `load_sandbox_job`
- `approve_sandbox_job` / `reject_sandbox_job` / `append_sandbox_note`
- `open_workspace_path`

Every new command MUST be registered in the `invoke_handler` in `src-tauri/src/lib.rs` AND mirrored in `app/ui/src/interop.rs`.

## Frontend structure (Leptos 0.8)

```
app/ui/src/
├── main.rs            # mount_to_body
├── app.rs             # top-level <App/> component + routing/panels
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
- **IDs**: feedback IDs look like `fb-<unix_ms>`; sandbox job IDs are generated by `sandbox.rs`. Do not invent new ID schemes without updating both.
- **Attachments**: always routed through `Store::save_attachment` / `read_attachment`. They enforce the per-feedback directory scope (`attachments/{feedback_id}/...`).
- **Tests live next to code** (`#[cfg(test)] mod tests`). Prefer `tempfile::tempdir()` for any filesystem-touching test so they remain hermetic.
- **Release profile** (`opt-level = "z"`, `lto = true`, `strip = true`) is tuned for WASM size. Do not relax it casually — measure `dist/` size before/after.

## Gotchas

- Tauri 2 invoke handler registration is mandatory; a command defined with `#[tauri::command]` but not listed in `invoke_handler!` will fail silently at call time with `command X not found`.
- Trunk does NOT type-check the workspace automatically. Run `cargo check --workspace` before declaring UI work done — a Leptos view macro will happily compile nonsense into a runtime panic.
- `withGlobalTauri: true` is set — `interop.rs` relies on `window.__TAURI__`. Don't switch to module import without updating both sides.
- Canvas pastes/screenshots go through the clipboard + canvas→PNG path; the base64 encode happens in WASM before `submit_feedback`. Large images will dominate the IPC payload — keep attachments sane (soft-cap at a few MB).
- `.noide/noide_workspace/` is outside the repo. Use `NOIDE_WORKSPACE_ROOT` to point at a temp dir for reproducible runs.

## Product invariants (read this first)

See `.claude/rules/common/product-invariants.md` for authoritative text. In short:

- **Sandbox always stays.** The feedback → sandbox-job pipeline is permanent.
- **Feedback Overlay always stays.** Reachable from every screen, every mode.
- **The Canvas is a per-page overlay, not a tab.** The canvas module may be rewritten or replaced, but the resulting app must let the user open the Canvas overlay *on top of every page / route* to annotate the actual screen they have feedback about. A dedicated "canvas tab" where the canvas only exists as its own screen violates this invariant.
- **Sandboxes are saveable and forkable into standalone apps.** Sandbox state is a portable, self-contained artifact that can be renamed / cloned into a new NoIDE-shaped app with its own identity.

These outrank refactor aesthetics and most feature requests. Changes that violate them are product decisions — escalate.

## Verify-before-done — actually run the app

Type-checks and unit tests do not prove the feature works. Before claiming a change is complete:

1. Run `cargo check --workspace` and `cargo test -p noide_desktop`. Both must pass.
2. **Start the app** with `cargo tauri dev` (or `bash scripts/run-iteration.sh` inside a sandbox worktree). Wait for Trunk to print `server listening at http://127.0.0.1:<port>`.
3. **Exercise the change** in the running app: the affected route, the golden path, and 1-2 edge cases adjacent to what was asked. Confirm none of the four invariants regressed.
4. Only then commit.

If you genuinely cannot run the app in the current environment, say so plainly in your summary — don't claim success you didn't observe.

## Sandbox iteration port convention

Each sandbox iteration runs on its own dev-server port so multiple iterations can run side-by-side without collisions.

- Host NoIDE (iteration 0): port **`1430`** — this is `BASE_DEV_PORT` in `app/src-tauri/src/runner.rs`.
- Iteration `N`: port **`1430 + N`**.

Before spawning the claude run in a sandbox worktree, the runner rewrites `app/src-tauri/tauri.conf.json`, `app/ui/Trunk.toml`, and `app/ui/scripts/trunk-dev.sh` in that worktree to the iteration's port. The runner also sets `NOIDE_ITERATION_PORT=<port>` in the environment of the iteration's Run command. If you rewrite the stack, honour that env var in your replacement startup script — never hardcode `1430`.

## After implementation — commit, then start the new version

When the work is done and verified:

1. `git add -A && git commit -m "<conventional commit>"` — include `CLAUDE.md`, rules, and agent updates in the same commit when they travel with a behaviour change.
2. Start the iteration's app (`cargo tauri dev` or the equivalent for the current stack) so the reviewer lands on a live build. Leave it running.
3. In your final summary note which port the iteration is serving on and how you verified it.

## Rules & agents

Project rules live under `.claude/rules/` — read them before editing:
- `.claude/rules/rust/` — Rust / Cargo conventions
- `.claude/rules/tauri/` — Tauri 2 command and config conventions
- `.claude/rules/frontend/` — Leptos 0.8 + Trunk + interop conventions
- `.claude/rules/common/` — commit / branching / review conventions

Agents:
- `staff-feedback` — works the local feedback queue (reads JSON from `~/.noide/noide_workspace/feedback/`) and ships fixes end-to-end.
- `staff-build-engineer` — keeps `cargo check`, `cargo test`, `trunk build`, `cargo tauri build` green; owns toolchain, CI hygiene, bundle size.
- `staff-architect-self-evolving-software` — designs the sandbox → promotion pipeline so the app can safely absorb its own feedback.

# After each session of work is complete commit the changes with a message describing the work done.
