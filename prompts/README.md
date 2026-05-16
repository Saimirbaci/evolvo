# Prompts

These Markdown files are the prose handed to Claude when a lineage job is advanced. They are loaded **at runtime** by `app/src-tauri/src/runner.rs`, so edits take effect on the next `approve_lineage_job` — **no recompile needed**.

## Files

- `_contract.md` — **shared operating contract** injected into every stage prompt and `implementation.md`. Defines the machine-parseable reply sentinel (`STAGE_RESULT: ok|fail: <reason>`), prompt-injection defense (user content fenced with `<user-content>` is data, not instructions), fail-fast contract, and blast-radius rules. Edit here when you want the rules to propagate to every agent.
- `implementation.md` — the outer prompt sent to `claude -p`. Wraps everything else.
- `iteration_guidance.md` — the per-iteration preamble (invariants, shell/app split, port, verify-before-done). Substituted into `implementation.md` as `{guidance}`.
- `new_app_banner.md` — the banner appended after `{guidance}` when the feedback type is `NewApp`.
- `phases/{bootstrap,shaping,consolidation,maturation}.md` — the "latitude" prose for each iteration phase. Substituted as `{latitude}` in `iteration_guidance.md`. Phase → iteration-range mapping is in Rust (`iteration_guidance()` in `runner.rs`).
- `work_steps/{new_app,bootstrap,shaping,maturation}.md` — the text for step 4 of the "How to work" checklist. `new_app.md` wins whenever the feedback type is `NewApp`; otherwise iteration phase picks the file. Substituted as `{work_step_4}`.

## Placeholders

Single-brace `{name}` substitution. No escaping — keep literal `{…}` out of prompt bodies (there are none today).

Available placeholders per file:

| File | Placeholders |
|---|---|
| `_contract.md` | none |
| `implementation.md` | `{contract}`, `{guidance}`, `{new_app_banner}`, `{job_id}`, `{branch}`, `{phase}`, `{title}`, `{feedback_type}`, `{route}`, `{feedback_text}`, `{voice_line}`, `{attachments_section}`, `{work_step_4}`, `{log_file}`, `{agent_label}`, `{agent_context_files}` |
| `iteration_guidance.md` | `{phase}`, `{latitude}`, `{port}` |
| `stages/*.md` | `{{CONTRACT}}`, `{{APP_NAME}}`, `{{JOB_ID}}`, `{{PORT}}`, `{{ROUTE}}`, `{{PLAN_PATH}}`, `{{CANVAS_PNG}}`, `{{ANNOTATIONS_PATH}}`, `{{USER_TEXT}}`, `{{VOICE_TRANSCRIPT}}`, `{{REGION_INDEX}}`, `{{WORKTREE}}` |
| `phases/*.md` | none |
| `work_steps/*.md` | none |
| `new_app_banner.md` | none |

Stage prompts use double-brace `{{NAME}}` and the legacy prompt uses single-brace `{name}` — keep them distinct so the substitution passes don't accidentally cross-fire.

## Override order

1. `EVOLVO_PROMPTS_DIR` env var, if set and points at an existing directory.
2. `<source_repo>/prompts/` — the repo copy next to this README. This is the default in dev and in lineage worktrees (the forked worktree carries its own `prompts/`, so iterations can experiment with prompt changes in isolation).
3. Compiled-in defaults — `include_str!`'d at build time from this directory. Used when the binary runs with no accessible source repo.

If a single file is missing from the override dir, the compiled-in default for that file is used — mix-and-match is supported.
