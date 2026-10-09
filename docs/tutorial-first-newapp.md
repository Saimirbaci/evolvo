# Tutorial: from install to your first promoted iteration

This tutorial walks you from a fresh clone to a running, promoted iteration of
a NewApp. It takes about 20 minutes if prerequisites are already installed, or
around 45 minutes if you are installing Rust, Tauri CLI, and Trunk for the
first time.

By the end you will have:

- Booted Evolvo and seen the welcome screen.
- Drawn on the canvas overlay and submitted your first piece of feedback.
- Watched an agent run produce an iteration.
- Promoted that iteration.
- Submitted a second piece of feedback inside the iteration's running app.
- Forked the lineage into a standalone app.

---

## Step 1 — Install prerequisites

Follow [`docs/INSTALL.md`](INSTALL.md) in full before continuing. The tutorial
assumes:

- `cargo tauri dev` works from `app/src-tauri/`.
- At least one agent CLI (`claude`, `codex`, or `grok`) is on your PATH.
- You are on macOS (Linux works too; Windows is untested).

---

## Step 2 — Clone and boot

```bash
git clone https://github.com/saimirbaci/Evolvo
cd Evolvo/app/src-tauri
cargo tauri dev
```

The first build takes 2–5 minutes. You will see Trunk print:

```
server listening at:
    http://127.0.0.1:1530/
```

Shortly after, the Evolvo desktop window opens.

**What you see:** a clean screen with the headline "Welcome to Evolvo" and a
subtitle reminding you about the button in the bottom-right corner. That button
is the Feedback FAB — the entry point to everything.

> **If the window stays blank:** a stale `trunk` process from a previous run
> may be holding port 1530. See the troubleshooting section of INSTALL.md to
> kill it.

---

## Step 3 — Open the canvas overlay and draw

Hover over the circle button in the **bottom-right corner**. A label pill
reading **"Feedback"** appears for a moment. The button's full tooltip reads
"Open canvas + send feedback".

**Click it.**

Two things happen simultaneously:

1. A transparent drawing surface drops over the entire window. The page behind
   it stays visible — you are annotating what you see, not a blank canvas.
2. A feedback submission panel slides in from the right.

**Try drawing something.** Use your mouse (or trackpad) to sketch on the page.
The pencil tool is active by default. Your strokes appear in the overlay colour
while the welcome screen content is still readable underneath.

You can also:
- Switch tools in the toolbar at the top of the canvas (pencil, shapes, text,
  eraser, undo).
- Paste a screenshot from the clipboard — it lands on the canvas.
- Click the microphone icon in the toolbar to record a voice note.

---

## Step 4 — Fill out the feedback panel and submit

With the canvas overlay open, look at the panel on the right.

### Choose a feedback type

The chip row shows: **New App · Feature · Improvement · Bug · Confusion ·
Compliment**. Select **New App** — you are asking the agent to build an
application from scratch.

When you select New App, an **Auto-evolve** checkbox appears below the chips
and is pre-checked. Leave it checked — this tells Evolvo to start the agent run
automatically after submission without waiting for you to click Evolve manually.

An **App screens** storyboard also appears. The current drawing is Screen 1.
Use **+ Screen** to sketch additional pages, give each one a name, route, and
short description, or duplicate a screen to preserve a shared layout. The
thumbnail strip lets you switch and reorder screens. Evolvo submits the full
ordered storyboard as one lineage job, so the agent can plan the routes and
navigation together instead of guessing unseen pages.

### Choose an agent

Below the chips is an agent row. Chips whose CLI is not installed are greyed
out. Click the one you want to use (Claude Code is the default and most
thoroughly tested).

### Write your request

In the text field, describe what you want built. For this tutorial, type
something concrete, for example:

```
Build a minimal Pomodoro timer: a 25-minute countdown, a Start/Pause button,
and a Reset button. Show the remaining time in large text.
```

### Submit

Click **Submit feedback**. The panel closes, the canvas overlay fades, and you
are back to the welcome screen.

---

## Step 5 — Open the Lineage tab and watch the job advance

Click the **Lineage** item in the app bar (or the "≡" navigation at the top
left). You will see a row for the feedback you just submitted. The job status
reads **pending** initially.

Because you left Auto-evolve checked, the status transitions to
**implementing** within a few seconds — the agent has been launched in an
isolated git worktree.

### What is happening behind the scenes

The runner has:
1. Created a git worktree at
   `~/.evolvo/evolvo_workspace/lineage_workspaces/<job-id>/`.
2. Symlinked context files so the agent can read `CLAUDE.md` / `AGENTS.md`.
3. Spawned the agent CLI with the structured prompt that includes your text and
   the canvas screenshot.

Because you submitted a **New App** type with a canvas annotation, Evolvo
routes this through the **multi-stage pipeline** (seven stages: backend_plan →
backend_impl → frontend_plan → frontend_impl → e2e_plan → e2e_impl →
final_review). The Lineage detail panel shows each stage and its status as
they complete.

> **Note:** A complex NewApp run takes 5–15 minutes depending on the agent and
> the scope of the request. A simple single-session run (Bug, Improvement, etc.)
> typically finishes in 2–5 minutes.

### While you wait

You can watch the live log stream by expanding the job row and clicking **View
log**. The log updates in real time as the agent writes to `claude.log`
(or the equivalent for your backend).

When all stages succeed, the status flips to **build_ready**. The Lineage tab
shows a **Run** button and an **Evolve** button.

---

## Step 6 — Run the iteration and verify it

Click **Run**. Evolvo launches the iteration's app on its own port
(iteration 1 runs on port `1531`). A browser or system launcher opens
`http://127.0.0.1:1531`.

**What to check:**

- Does the app match what you asked for? For the Pomodoro example: is there a
  countdown, a Start/Pause button, a Reset button?
- Does it behave correctly? Start the timer, let it tick for a few seconds,
  pause it, reset it.
- Are there any console errors? (Open DevTools if needed.)

If something is wrong, close the run window and click **Reject** in the Lineage
tab. The job moves to `rejected` (terminal). You can then submit a new piece of
feedback describing what was wrong — the lineage records your decision.

If it looks good, proceed to step 7.

---

## Step 7 — Promote the iteration

Back in the Evolvo Lineage tab, click **Evolve** on the `build_ready` job.

Evolvo merges the worktree into your source. The job status flips to
**promoted**. This is the terminal success state — the iteration is now part
of your app's history.

The Lineage tab shows the promoted node in the tree. The job's notes record the
full timeline: feedback submitted, run requested, stages completed, promoted.
This is the lineage: a permanent, auditable record of how this version of the
app came to be.

---

## Step 8 — Submit a second piece of feedback from inside the iteration

Switch to the running iteration's window at `http://127.0.0.1:1531` (or
relaunch it via **Run** if you closed it). This is your Pomodoro app — a new
Evolvo iteration running its own dev server.

Open the Feedback FAB in *that* window (the same ✎ button, bottom-right
corner). You are now submitting feedback *about the Pomodoro app itself*,
annotating the actual Pomodoro UI.

Draw an arrow pointing to the timer display and type:

```
Make the timer digits larger and change the font to a monospace style.
```

Submit. Switch back to the host Evolvo window (port `1530`) and open the
Lineage tab. You will see a new job appear as a **child** of the iteration you
just promoted. The lineage tree branches: iteration 1 is the parent; iteration
2 is its child.

Click **Evolve** on the child job to start the agent run. The same review
cycle applies: wait for `build_ready`, click **Run** (now on port `1532`),
verify, promote.

This is the core loop. Every promoted iteration is a node you can return to,
inspect, and branch from.

---

## Step 9 — Fork the lineage into a new app

Any promoted job can be exported as a `.evolvo-bundle` and imported into a
fresh workspace — this is what "lineages are forkable" (I-P4) means in
practice.

1. In the Lineage tab, hover over the job row you want to fork. An overflow
   menu icon (**⋯**) appears at the right edge.
2. Click **⋯ → Fork…**
3. Evolvo prompts you for a destination directory. Enter a path on your disk
   (for example `~/dev/pomodoro-app`).

Evolvo creates a `.evolvo-bundle` tar archive and writes it to the destination.
The bundle contains the `LineageJobRecord`, the original `FeedbackRecord`, all
attachments (canvas PNG, voice file, pasted images), and a `manifest.json`.
Host-specific paths are stripped — the bundle has no reference to your machine.

To continue the forked lineage in a separate workspace:

```bash
# Point a new Evolvo instance at the empty workspace
EVOLVO_WORKSPACE_ROOT=~/dev/pomodoro-workspace cargo tauri dev
```

In the new instance, click **Open bundle…** in the Lineage sidebar, enter the
path to the `.evolvo-bundle` file, and confirm the target workspace root. The
importer unpacks the records and attachments. The imported lineage is now the
history of the new app — independent of the original. You can evolve it, rename
it, or hand it to a collaborator.

---

## Summary

You have completed the full Evolvo loop:

| Step | What happened |
|---|---|
| Boot | Evolvo created `~/.evolvo/evolvo_workspace/` and served the UI at port 1530 |
| Feedback | You drew on the canvas overlay and submitted a NewApp request |
| Lineage job | Evolvo auto-created a job and routed it through the multi-stage pipeline |
| Agent run | The agent built a Pomodoro app in an isolated git worktree |
| Review | You ran the iteration on port 1531 and verified it worked |
| Promote | You approved the iteration; its history is now permanent in the lineage |
| Iteration 2 | You submitted feedback from inside the running app; the lineage branched |
| Fork | You exported the lineage as a `.evolvo-bundle` and started a new workspace |

The lineage is the product. The `.evolvo-bundle` you created is a portable,
self-contained record of every decision that led to the Pomodoro app — who
asked for what, which agent ran, what it produced, and whether you approved it.
You can pick it up on another machine, hand it to a collaborator, or use it as
the starting point for a different product.

---

## What to read next

- [`docs/concepts.md`](concepts.md) — the full lineage model, invariants, and
  agent backend reference.
- [`CONTRIBUTING.md`](../CONTRIBUTING.md) — how to propose iterations and
  submit PRs.
- [`docs/INSTALL.md`](INSTALL.md) — full prerequisites matrix if you hit
  build issues.
