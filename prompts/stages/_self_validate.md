# Self-validate before you finish

The deterministic validator that gates this stage is available to you as a
CLI — the exact same checks the pipeline will run after you exit. Run it
yourself and iterate until it passes:

```bash
"$EVOLVO_VALIDATE_BIN" validate --stage {{STAGE_SLUG}} \
  --job-dir {{JOB_DIR}} --worktree {{WORKTREE}}
```

If `$EVOLVO_VALIDATE_BIN` is unset in your environment, fall back to
building it from the worktree (slower, same checks):

```bash
cargo run -p evolvo_desktop -- validate --stage {{STAGE_SLUG}} \
  --job-dir {{JOB_DIR}} --worktree {{WORKTREE}}
```

Rules:

- Exit code 0 means the stage will be accepted. Exit code 1 prints the
  failed checks (lines starting with `FAIL`) — fix exactly those and run
  it again. Exit code 2 is a usage/I-O error: re-read the command.
- The validator only reads `plan.json` and the worktree. Running it has no
  side effects, so run it as often as you like.
- Do **not** end the session with `STAGE_RESULT: ok` while the validator
  still rejects — the pipeline will fail the stage regardless of what you
  claim. If after 3 self-fix cycles it still rejects, emit
  `STAGE_RESULT: fail: <name of the first failing check>` and stop.
- Never edit the validator's own source (`app/src-tauri/src/validators.rs`)
  to make it pass — the pipeline runs the host's copy, not yours, and the
  discrepancy will be treated as a failed stage.
