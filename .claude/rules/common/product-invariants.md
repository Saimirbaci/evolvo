# Product invariants — NoIDE

These are **load-bearing product invariants**. They outrank aesthetic preferences, refactor opportunities, and most feature requests. Every agent and skill in this repo must respect them. Breaking one is a product decision, not a code change — escalate to the user.

## I-P1. Sandbox always stays

The **sandbox pipeline** (feedback → sandbox job → reviewable/approvable state machine in `app/src-tauri/src/sandbox.rs`) is a first-class, permanent feature of NoIDE. It is never optional, never feature-flagged off, never removed, never "temporarily disabled" during a refactor. Any change that deletes, bypasses, or silently no-ops the sandbox is rejected on sight.

## I-P2. Feedback Overlay always stays

The **Feedback Overlay** — the in-app surface that lets the user capture feedback from anywhere in the app (the submission panel + whatever overlay/trigger invokes it) — is always present and always reachable, on every screen, in every mode. No hidden toggle, no "pro mode" that removes it, no route where it's unavailable. If a refactor would make the overlay unreachable from some state, the refactor is wrong.

## I-P3. The Canvas is a per-page overlay, not a tab

The Canvas is the drawing/annotation layer the user uses to give visual feedback. It is **an overlay that can be invoked on any page or route of the app**, drawn on top of whatever the user is currently looking at, and attached to the Feedback Overlay submission for that page.

The canvas implementation (today `app/ui/src/canvas.rs`) **may be removed, rewritten, split, or replaced**. The product guarantees are:

- **Every page / route** of the app under construction must support opening the Canvas overlay on top of it. There is no screen where the Canvas is unavailable.
- The Canvas is **never a standalone tab or dedicated route**. Making it its own tab (or the only screen that can be annotated) breaks this invariant — the user loses the ability to annotate the *actual* page they have feedback about.
- When the Canvas is open, the user can sketch, annotate, paste images, and submit that drawing as part of feedback *about the underlying page*. The route/context of the page being annotated must be recorded with the feedback.
- The Canvas overlay must be **discoverable from every page** (toolbar, keyboard shortcut, or floating trigger — the specific affordance can change; its ubiquity cannot).

A rewrite that makes the Canvas a separate tab, or that makes any page un-annotatable, is incomplete.

## I-P4. Sandboxes are saveable and forkable into standalone apps

A user can **save a sandbox** and **rename / fork it into another app**. This means:

- Sandbox state (jobs, notes, the feedback that fed them, associated attachments) is persistable as a self-contained artifact on disk.
- That artifact is portable — the user can take it and turn it into a new NoIDE-shaped app, with its own identity (name, workspace root), independent of the original.
- "Rename into another app" implies an export/clone operation that mints a new app identity, not an in-place mutation of the current one.

Any storage, state-machine, or workspace-layout change must preserve the ability to implement this. If the proposed change makes sandboxes non-portable (embeds host-specific paths, bakes in the current app name, loses the feedback↔job↔attachment linkage), it's rejected.

## What this means operationally

- **Every agent** treats Sandbox + Feedback Overlay as non-negotiable surfaces. Fixes, refactors, and rewrites must preserve them.
- **Every design proposal** that touches the sandbox state machine or the overlay must explicitly state how it preserves I-P1 through I-P4.
- **Canvas rewrites are allowed** — but the reviewer must verify the Canvas overlay is invokable on *every page/route* of the resulting app (not just one dedicated screen) before approving.
- **Sandbox portability** is a first-class requirement, not a future nice-to-have. New fields on `SandboxJobRecord` should be serializable and self-describing, not pointers into host state.
