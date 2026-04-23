# Git workflow — Evolvo

## Branches

- `main` is the only long-lived branch.
- Work on short-lived topic branches: `fix/<short>`, `feat/<short>`, `chore/<short>`.
- Never force-push `main`. Force-push on a topic branch is fine if the branch is yours.

## Commits

Conventional-commit style, single line, lowercase:

- `fix(<scope>): <short>`
- `feat(<scope>): <short>`
- `refactor(<scope>): <short>`
- `chore(<scope>): <short>`
- `test(<scope>): <short>`
- `docs(<scope>): <short>`

Scopes used in this repo:
- `host` — anything in `app/src-tauri/`
- `ui` — anything in `app/ui/`
- `store` — specifically `store.rs`
- `lineage` — lineage state machine
- `interop` — `app/ui/src/interop.rs`
- `config` — Tauri / Trunk / Cargo config
- `ci` — CI / scripts

When the commit closes a feedback row processed by `staff-feedback`:
```
fix(ui): align toolbar labels — feedback:a1b2c3d4
```

Commit bodies (when needed) describe **user-visible behavior change**, not the diff. Two short paragraphs max.

## Before committing

Every commit must leave the tree green:

```bash
cargo check --workspace
cargo test -p noide_desktop
cargo clippy -p noide_desktop -- -D warnings   # if host code changed
cargo check -p noide_ui --target wasm32-unknown-unknown   # if UI changed
```

For UI-visible changes, also run `cargo tauri dev` once and eyeball the flow — type-check is not the same as "it works".

## No `--no-verify`

If a pre-commit hook fails, fix the root cause. Never bypass.

## Never commit

- `~/.evolvo/noide_workspace/` contents (it's outside the repo anyway — but don't symlink it in).
- Real user feedback JSON. If you need a fixture, synthesize it.
- Secrets, tokens, signed bundle certs.
