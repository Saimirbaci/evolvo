# Product invariants — NoIDE

These are **load-bearing product invariants**. They outrank aesthetic preferences, refactor opportunities, and most feature requests. Every agent and skill in this repo must respect them. Breaking one is a product decision, not a code change — escalate to the user.

## I-P1. Sandbox always stays

The **sandbox pipeline** (feedback → sandbox job → reviewable/approvable state machine in `app/src-tauri/src/sandbox.rs`) is a first-class, permanent feature of NoIDE. It is never optional, never feature-flagged off, never removed, never "temporarily disabled" during a refactor. Any change that deletes, bypasses, or silently no-ops the sandbox is rejected on sight.

## I-P2. Feedback Overlay always stays

The **Feedback Overlay** — the in-app surface that lets the user capture feedback from anywhere in the app (the submission panel + whatever overlay/trigger invokes it) — is always present and always reachable, on every screen, in every mode. No hidden toggle, no "pro mode" that removes it, no route where it's unavailable. If a refactor would make the overlay unreachable from some state, the refactor is wrong.

## I-P3. The drawing board is always reachable

The canvas implementation (today `app/ui/src/canvas.rs`) **may be removed, rewritten, split, or replaced**. The product guarantee is not the current implementation — it is that the user can, at any time, **return to a drawing board**. Whatever the UI shell looks like, there is always a visible, discoverable way to get back to a blank drawing surface. A rewrite that loses this affordance is incomplete.

## I-P4. Sandboxes are saveable and forkable into standalone apps

A user can **save a sandbox** and **rename / fork it into another app**. This means:

- Sandbox state (jobs, notes, the feedback that fed them, associated attachments) is persistable as a self-contained artifact on disk.
- That artifact is portable — the user can take it and turn it into a new NoIDE-shaped app, with its own identity (name, workspace root), independent of the original.
- "Rename into another app" implies an export/clone operation that mints a new app identity, not an in-place mutation of the current one.

Any storage, state-machine, or workspace-layout change must preserve the ability to implement this. If the proposed change makes sandboxes non-portable (embeds host-specific paths, bakes in the current app name, loses the feedback↔job↔attachment linkage), it's rejected.

## What this means operationally

- **Every agent** treats Sandbox + Feedback Overlay as non-negotiable surfaces. Fixes, refactors, and rewrites must preserve them.
- **Every design proposal** that touches the sandbox state machine or the overlay must explicitly state how it preserves I-P1 through I-P4.
- **Canvas rewrites are allowed** — but the reviewer must verify the "return to drawing board" affordance is intact before approving.
- **Sandbox portability** is a first-class requirement, not a future nice-to-have. New fields on `SandboxJobRecord` should be serializable and self-describing, not pointers into host state.
