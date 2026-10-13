# Contributing to Evolvo

Thank you for contributing. This document covers how to file feedback,
propose iterations, write PRs, and work with the staff agents that maintain
this repo.

Evolvo is a provenance-native app builder — vibe coding with a memory. The
lineage is the product. Contributions that advance that story are welcome;
contributions that complicate it without paying for the complexity are not.
When in doubt, smaller is better.

---

## Code of Conduct

All participation is governed by the
[Contributor Covenant v2.1](CODE_OF_CONDUCT.md). The short version: be
direct, be honest, be kind. The long version is in the CoC.

---

## How to file feedback

**The right way: use Evolvo itself.**

Evolvo is dogfooded against its own development. If you have feedback about
a feature, a UX rough edge, or a behaviour you don't understand, open the
Feedback FAB (bottom right of the running app), sketch on the page you're
looking at, and submit. Your feedback becomes a lineage job that a maintainer
can review and evolve.

This is not a rhetorical ask. The lineage of how Evolvo became what it is
lives in `~/.evolvo/evolvo_workspace/` and is built from submissions like
yours. Using the feedback loop is the most direct contribution you can make.

**Exception: Evolvo won't start at all.**

If you hit a bug that prevents Evolvo from launching, you can't dogfood it.
Open a [GitHub Issue](https://github.com/saimirbaci/Evolvo/issues/new) with:

- OS and Rust version (`rustc --version`, `cargo tauri --version`,
  `trunk --version`)
- The full terminal output of `cargo tauri dev`
- What you expected vs. what happened

Use the **Bug report** issue template when it exists; otherwise include those
three fields in plain text.

---

## How to propose an iteration

An *iteration* is a unit of change: one feedback row → one agent run → one
reviewable diff. To propose an iteration:

1. **Write a brief** in this format:
   - **Why now** — what problem does this solve, and why is it urgent
     relative to what's already planned?
   - **What ships** — one concrete thing that is done when this iteration
     is done. Not "improve X" — "X works like Y, confirmed by Z."
   - **Out of scope** — one sentence on what this deliberately does not
     touch.
   - **Success criteria** — how a reviewer verifies "done" without
     asking the author.

2. **Open a GitHub Discussion** (category: *Ideas*) with the brief as the
   body. Discussions are the right venue because iterations are proposals,
   not done things — they need product review before engineering starts.

3. If a maintainer approves the brief, it will be added to the lineage
   queue and an agent run will produce a draft PR. You are welcome to
   implement it yourself instead; if so, open a Draft PR with the brief
   and tag it `iteration-proposal`.

---

## How PRs work

### Branch naming

```
fix/<short-slug>
feat/<short-slug>
chore/<short-slug>
docs/<short-slug>
```

### Commit style

[Conventional commits](https://www.conventionalcommits.org/), single line,
lowercase. The full convention is in
[`.claude/rules/common/git-workflow.md`](.claude/rules/common/git-workflow.md).

Scopes used in this repo:

| Scope | Covers |
|---|---|
| `host` | `app/src-tauri/` |
| `ui` | `app/ui/` |
| `store` | `store.rs` |
| `lineage` | lineage state machine |
| `interop` | `app/ui/src/interop.rs` |
| `config` | Tauri / Trunk / Cargo config |
| `docs` | documentation |
| `ci` | CI / scripts |

### The six gates

Every PR must pass these before merge. Run them yourself before opening;
the CI check (when it exists) will also run them.

```bash
# 1. Workspace typecheck
cargo check --workspace

# 2. Host unit tests
cargo test -p evolvo_desktop

# 3. Host clippy (deny warnings)
cargo clippy -p evolvo_desktop -- -D warnings

# 4. WASM typecheck
cargo check -p evolvo_ui --target wasm32-unknown-unknown

# 5. cargo fmt (no custom config — default rustfmt)
cargo fmt --all -- --check

# 6. Manual smoke test
# Run cargo tauri dev, exercise the changed surface, confirm the four
# product invariants did not regress (see below).
```

If you cannot run `cargo tauri dev` in your environment (e.g. CI without a
display), say so explicitly in the PR description — do not claim the smoke
test passed when it didn't.

### Product invariants

Every PR must preserve these four invariants (authoritative text in
[`.claude/rules/common/product-invariants.md`](.claude/rules/common/product-invariants.md)):

1. **I-P1. Lineage always stays.** The feedback → lineage job pipeline is
   permanent and may not be disabled, feature-flagged off, or silently
   bypassed.
2. **I-P2. Feedback Overlay always stays.** The canvas + feedback submission
   surface is always reachable from every page, every mode.
3. **I-P3. The Canvas is a per-page overlay, not a tab.** The canvas must
   be invokable on top of every route, not only a dedicated canvas screen.
4. **I-P4. Lineages are saveable and forkable.** Every change to the storage
   or state-machine format must preserve the ability to export a lineage as
   a portable `.evolvo-bundle` and import it into a fresh workspace.

A PR that breaks any of these will be rejected regardless of how good the
rest of the change is. If you believe a violation is necessary, open a
Discussion first — invariant changes are product decisions, not code decisions.

### Adding a Tauri command

Every new command requires changes in exactly three places (missing any one
causes a silent 404 at runtime):

1. Define with `#[tauri::command]` in `app/src-tauri/src/commands.rs`.
2. Register in `generate_handler!` in `app/src-tauri/src/main.rs`.
3. Add a typed `async fn` wrapper in `app/ui/src/interop.rs`.

The full convention is in
[`.claude/rules/tauri/commands.md`](.claude/rules/tauri/commands.md).

### Rust conventions

See [`.claude/rules/rust/conventions.md`](.claude/rules/rust/conventions.md).
Key points:

- No `.unwrap()` / `.expect()` in production paths. Tests may use them;
  commands return `Result<T, String>`.
- All user-supplied filenames go through `store::sanitise_filename`.
  Path traversal is the only realistic local attack surface; take it
  seriously.
- New UI (WASM) dependencies must be justified by size. Measure
  `dist/*.wasm` before and after.

### Leptos / frontend conventions

See [`.claude/rules/frontend/leptos.md`](.claude/rules/frontend/leptos.md).
Key points:

- `shell.rs` is invariant chrome — do not add new permanent UI surfaces
  here without a product discussion. NewApp work goes in `app.rs`.
- `interop.rs` is the only file that may call
  `window.__TAURI__.core.invoke`. Other modules receive closures or
  signals.
- Every icon-only button needs `aria-label` and `title`.

---

## Working with the staff agents

The repo ships four Claude Code agent personas under `.claude/agents/`.
They are invoked in Claude Code sessions (prefix with `@`); contributors
with Claude Code access can dispatch them on their local branches.

| Agent | Owns | When to dispatch |
|---|---|---|
| `staff-product-manager` | Product brief, iteration backlog, UX review, voice | You have an iteration idea and want a brief shaped; or you shipped something and want a product lens review |
| `staff-feedback` | Implements feedback from the workspace end-to-end | You want to process the feedback queue, close a specific feedback row, or implement a well-specified fix |
| `staff-build-engineer` | Cargo workspace, Trunk, Tauri bundle, CI, toolchain | The build is broken, a dep needs upgrading, CI is red, bundle size spiked |
| `staff-architect-self-evolving-software` | Lineage state machine, promotion policy, storage-format evolution, reversibility | You are changing `lineage.rs`, `store.rs`, the state machine, or any mechanism that lets the app write to its own source |

Agents are not oracles — they are opinionated colleagues with defined
ownership. Routing a lineage state-machine question to `staff-feedback`
will get a polite redirect; route it to `staff-architect` instead.

---

## License

By contributing, you agree that your contributions will be licensed under
the same license as the project. See [LICENSE](LICENSE) for details.

---

## What we do not need

- CLA — no CLA required.
- Triage SLAs — we do not have the volume yet to commit to a response time.
  We will try to respond to issues and PRs within a week, but cannot
  guarantee it.
- Sponsor / funding asks — not accepting sponsored contributions at this time.
