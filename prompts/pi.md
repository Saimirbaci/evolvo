# Local-model delegation (pi + Ollama)

You are running this lineage iteration as the **supervisor**. The user
opted into local-model delegation, so a second worker — `pi` driving a
local Ollama model (typically `gemma4`) — is available for boilerplate
sub-implementations. The point is to save your tokens on work that
doesn't need your reasoning. **You still own planning, cross-file
decisions, and final review.**

## How to invoke the worker

The runner exposes a wrapper at `$EVOLVO_DELEGATE_BIN`. Call it via the
`Bash` tool with a single argument: an absolute path to a spec file you
just wrote into the worktree (e.g. under `.evolvo/delegations/`).

```
$EVOLVO_DELEGATE_BIN /tmp/spec-create-task-handler.md
```

The wrapper:
- Streams the worker's transcript into the per-job `pi.log`.
- Returns rc=0 on success, rc=4 if the worker errored OR the post-delegation
  `cargo check` gate found compile errors, rc=124 on timeout.
- Has a 180s timeout by default. Subtasks that need longer are too big to
  delegate — keep them yourself.
- **Runs `cargo check --workspace` automatically** when pi touched any
  `.rs` file, before returning. If the check fails, the wrapper exits
  rc=4 and the compiler diagnostic appears on stderr (and in `pi.log`).
  You won't get a green delegation that doesn't compile.

When the gate fails, **take the task back**: read the worker's log,
revert its changes (`git checkout -- <files>`) and either retry with a
tighter spec or implement it yourself. Do **not** chain delegations to
patch what a previous delegation broke — you'll burn the iteration.

If the spec deliberately targets non-Rust files (e.g. styles.css, a
markdown doc, a JSON fixture), set `EVOLVO_DELEGATE_SKIP_CHECK=1` in the
environment when invoking the wrapper to skip the gate. Don't abuse this
— the gate is the only thing protecting you from a confidently-wrong
worker.

## When to delegate

Delegate **only** when *all four* are true:

1. The spec fits in fewer than ~40 lines of plain English.
2. The output is one or two self-contained files (or a tight diff to
   one file). No cross-cutting refactors.
3. You can write down exact signatures and an acceptance assertion.
   "Make function `X(a: A, b: B) -> Result<C, String>` such that …" is
   delegate-shaped. "Refactor module Y to use the new pattern" is not.
4. The cost-of-mistake is local. A miswritten boilerplate handler fails
   `cargo check` and you take it back. A misdesigned Tauri command is
   load-bearing — keep that yourself.

Good fits:
- CRUD command + handler implementations once you've written the
  signature and decided the storage layout.
- Test scaffolds from a known signature (one happy-path + one edge case).
- Repetitive transformations (rename a field across several files where
  every occurrence is mechanical).
- Pure-function modules (a parser, a formatter, a small data transform)
  with a clear input/output contract.
- Filling in a derive-heavy data type from a list of fields.

## When NOT to delegate

- **Architectural decisions.** The plan stages of the multi-stage
  pipeline (`*_plan`) are supervisor-only. The worker has no context
  for trade-offs.
- **Cross-file refactors** that need to read more than two files of
  context to do correctly. The worker will hallucinate plausible-looking
  but inconsistent code.
- **Anything in `lineage.rs`, `runner.rs`, `stages.rs`, `commands.rs`,
  `store.rs`, `types.rs`.** These are the supervisor's job — the worker
  will guess wrong about Tauri command shapes, the lineage state
  machine, or the storage envelope.
- **Tricky lifetime / type-inference work** in Rust. Local models are
  weaker here.
- **Anything where the spec is longer than the resulting code.** That
  means you're already doing the hard part — just write it.

## Spec format

Every spec file you write must include:

```
# <one-line task title>

## Target file(s)
- <absolute path 1>
- <absolute path 2>

## Required imports / context
- crate::types::Foo
- existing helper `bar()` at <path>

## Signature(s)
```rust
fn create_task(input: CreateTaskInput) -> Result<Task, StoreError>
```

## What it must do
<5–15 sentences of plain English. Cover the happy path and any edge
cases the worker won't infer.>

## Acceptance assertion
After this lands, `cargo check --workspace` passes AND
`cargo test -p evolvo_desktop -- create_task` passes.
```

## Hard limits

- **Max 5 delegations per stage.** Past that, the savings stop and the
  drift starts. If you hit the cap, finish the rest yourself.
- **One subtask per spec file.** No multi-task specs — they read like
  prose to the worker and produce drift.
- **Always log a lineage note** describing what you delegated, why, and
  the rc. Use the project's note-append convention (see `CLAUDE.md`).
  This is what makes the savings auditable.

## When delegation is unavailable

If `$EVOLVO_DELEGATE_BIN` is unset, or the wrapper returns rc=3 ("pi not
on PATH"), or rc indicates Ollama is unreachable: ignore delegation and
do the work yourself. Do not block on local-worker availability — the
user opted in, but the lineage must still land.
