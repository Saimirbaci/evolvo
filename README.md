# Evolvo

**Evolvo turns feedback into iterations of your app, and keeps the lineage.**
Sketch on the page you're looking at — draw, type, paste, speak. An agent
of your choice (Claude Code, Codex, or Grok — local, BYOK) turns
each piece of feedback into a new iteration. Every iteration is a node on
a tree you can review, run, fork, and rename into an app of its own. Local
by default — nothing leaves your machine, nothing ships without your click.
The lineage is the product: vibe coding with a memory.

> **Status: alpha.** The core loop (feedback → lineage job → agent run →
> review → promote) is stable. Multi-agent backends, the bundle export/import
> flow, and the canvas overlay are in daily use. Breaking changes to workspace
> JSON schemas will be called out in release notes. Not yet recommended for
> production use.

---

<!-- Replace this block with an animated GIF or screenshot showing:
     feedback panel open → agent run → iteration reviewed → promoted.
     Suggested size: 900px wide, < 3 MB.
     Place the asset at docs/images/demo.gif and uncomment the line below. -->
<!-- ![Evolvo loop: feedback → iteration → promote](docs/images/demo.gif) -->

---

## What it is

Evolvo is a **provenance-native app builder** — a desktop app (Tauri 2 +
Leptos/WASM) that wraps a feedback-to-iteration pipeline around whatever app
you're building. You give feedback on the running app using a transparent
canvas overlay that sits on top of every page. The feedback becomes a lineage
job. For a multi-page New App, add, name, duplicate, and reorder storyboard
screens in that same overlay; Evolvo sends the complete ordered flow to the
agent in one job. The lineage job runs an agent of your choice. The agent produces an
iteration. You review the diff, run the iteration locally, and — with one
click — promote it or reject it. The full history lives in
`~/.evolvo/evolvo_workspace/` as plain JSON files.

The lineage is the product. Other vibe-coding tools generate code and forget
the context. Evolvo records a portable, forkable tree of every decision —
who asked for what, which agent ran, what it produced, whether you approved
it. You can fork any node into a new app of its own, bundle the whole tree
into a `.evolvo-bundle`, and hand it to someone else to continue.

## Why it's different

**We are not Cursor / Replit / Lovable.**
Those are vibe-coding tools. Evolvo is the layer *around* them — we record
the lineage that vibe coding throws away. We integrate with the underlying
agent CLIs (Claude Code, Codex, Grok) as interchangeable backends;
we are not competing on editor polish or hosted model quality.

**We are not a Jira / Linear import.**
Feedback in Evolvo is not a ticket waiting for a human. It is a job that
flows through a reviewable, agent-driven pipeline. The PM and the engineer
are the same person, often the same click.

**We are not auto-promote.**
Every iteration that lands in your app is one you approved. The agent never
writes to your source without a human gate. This is the safety property the
category lacks — see [Safety](#safety) below.

## Quickstart

Full prerequisites (Rust, Tauri CLI, Trunk, wasm32 target, one or more agent
CLIs) are documented in [`docs/INSTALL.md`](docs/INSTALL.md) *(forthcoming)*.

**Fastest path if you already have the prereqs:**

```bash
git clone https://github.com/saimirbaci/Evolvo
cd Evolvo/app/src-tauri
cargo tauri dev
```

Trunk serves the UI at `http://127.0.0.1:1530` and Tauri opens the desktop
window automatically. The workspace is created at
`~/.evolvo/evolvo_workspace/` on first run.

**To use a different workspace root** (e.g. for a specific project):

```bash
EVOLVO_WORKSPACE_ROOT=/path/to/my-project/workspace cargo tauri dev
```

**To run a specific iteration's app** (after an agent has produced one):

```bash
# The Lineage tab shows the port for each iteration (base port + iteration N).
# Example for iteration 3:
cd ~/.evolvo/evolvo_workspace/lineage_workspaces/<job-id>
bash scripts/run-iteration.sh
```

## Safety

Evolvo runs locally by default — no cloud, no telemetry, no phone-home unless
you opt in. Every iteration the agent produces is gated by an explicit human
approval before it touches your app. The agent writes to a throwaway git
worktree; you promote the worktree to your source only after reviewing the
diff and clicking Evolve. Your source repo is never touched without that click.

The workspace is plain JSON files on your disk. You can read, audit, delete,
or back up every feedback record, every iteration, and every attachment without
touching Evolvo itself.

## Supported agent backends

Evolvo ships with three backends. Install any combination; the UI greys out
backends whose CLI is not on your `PATH`.

| Agent | CLI binary | Install |
|---|---|---|
| Claude Code | `claude` | [docs.anthropic.com/claude-code](https://docs.anthropic.com/en/docs/claude-code) |
| Codex CLI | `codex` | [github.com/openai/codex](https://github.com/openai/codex) |
| Grok CLI | `grok` | xAI Grok Build CLI (then `grok login`) |

All three read `CLAUDE.md` / `AGENTS.md` at the worktree root, so a single
project guide reaches every backend. Gemini CLI and Forge are retired: old
jobs that used them still load (shown as "(retired)") and retry/resume
with Claude Code.

## Workspace layout

```
~/.evolvo/evolvo_workspace/
├── feedback/              {id}.json — every piece of feedback you submitted
├── lineage_jobs/          {id}.json — the job created for each feedback row
├── attachments/{id}/      canvas.png or screen-NN.png, pasted images, voice.*
└── lineage_workspaces/    one git worktree per agent run
```

The full schema is in [`app/src-tauri/src/types.rs`](app/src-tauri/src/types.rs).
Lineage bundles (`.evolvo-bundle`) are tar archives of one job + its feedback +
attachments, with host paths stripped. Import them with the "Open bundle…"
button in the Lineage tab.

## Docs

- [`docs/positioning.md`](docs/positioning.md) — category claim, tagline, tone guide
- [`docs/INSTALL.md`](docs/INSTALL.md) — full prerequisites matrix *(forthcoming)*
- [`CLAUDE.md`](CLAUDE.md) — internal guide for contributors and agent runs

## Contributing

Contributions are welcome. The codebase guide for both humans and agents is
[`CLAUDE.md`](CLAUDE.md). Before opening a PR, run the full build contract:

```bash
cargo check --workspace
cargo test -p evolvo_desktop
cargo clippy -p evolvo_desktop -- -D warnings
cargo check -p evolvo_ui --target wasm32-unknown-unknown
```

A `CONTRIBUTING.md` with the full contribution workflow is forthcoming.

## Security

There is no network surface to attack — Evolvo has no server, no API, no cloud
backend. The attack surface is: the Tauri IPC bridge (commands are in
[`app/src-tauri/src/commands.rs`](app/src-tauri/src/commands.rs)), filename
handling (`sanitise_filename` in `store.rs`), and the agent CLI spawn in
`runner.rs`. Report vulnerabilities by opening a private GitHub Security
Advisory on this repository. A `SECURITY.md` is forthcoming.

## License

License forthcoming. Until a LICENSE file appears in this repository, the code
is source-available but not open-source: you may read and fork it for personal
use, but redistribution and commercial use require written permission from the
authors.
