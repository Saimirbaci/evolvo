# Evolvo positioning brief

Owner: staff-product-manager. This file is the input to every public artifact —
README, INSTALL, SECURITY, CONTRIBUTING, the landing page, the launch tweet.
If a downstream artifact disagrees with this brief, edit the brief first, then
the artifact. Locked 2026-05-02.

## What Evolvo is, in one sentence

Evolvo records the **lineage** of how your app became what it is — every piece
of feedback, every iteration, every fork — and lets you review, run, and ship
each one with a click.

## Category

**Provenance-native app builder** is the formal label (README, SECURITY,
INSTALL). It inherits credibility from data lineage / ML provenance and
translates it into a domain — app development — where it doesn't yet exist.

In marketing copy and conversational explanations we lead with the promise,
not the label: *"the lineage is the product."* "Provenance-native" is a
subhead, never the opening sentence — readers bounce off coined nouns and
stay for concrete verbs.

Backup framings if the primary doesn't survive outside review:
1. **Generational software lab** — biology metaphor, distinctive, marketing-
   surface friendly.
2. **Forkable app studio** — concrete and functional; "forkable" is a feature,
   not a worldview, so this is the explainer for skeptical engineers asking
   "what does it actually let me do," not the headline.

## Name

**Evolvo.** The metaphor (evolution → iterations → generations) is already
encoded; codebase, bundle id (`com.opsync.evolvo`), and repo URL are invested.
The name is not the problem — the category is.

## Headline paragraph (README first 100 words)

> **Evolvo turns feedback into iterations of your app, and keeps the lineage.**
> Sketch on the page you're looking at — draw, type, paste, speak. An agent
> of your choice (Claude Code, Codex, or Grok — local, BYOK) turns
> each piece of feedback into a new iteration. Every iteration is a node on
> a tree you can review, run, fork, and rename into an app of its own. Local
> by default — nothing leaves your machine, nothing ships without your click.
> The lineage is the product: vibe coding with a memory.

This paragraph does five things at once: opens with a verb (no coined-noun
bounce), names the three agent backends to satisfy the BYO-model reader,
reassures on the safety story before anyone asks, names the wedge (the
lineage), and lands the tagline as the closer. Every other artifact ladders
to this paragraph.

## Tagline

**"Vibe coding with a memory."**

Five words. Names the category we're parasitizing, names what it's missing,
claims the gap. Works on a t-shirt, in a tweet, in an HN title. Combative
toward the incumbents (Cursor, Replit, Lovable, Bolt, v0) by design — the
thing they throw away is the thing we keep.

Softer fallback for slide decks and enterprise pitch surfaces: *"Apps that
remember how they got here."*

## Three "we are not" claims

1. **We are not Cursor / Replit / Lovable.** Those are vibe-coding tools.
   Evolvo is the layer *around* them — we record the lineage that vibe coding
   throws away. We integrate with the underlying agent CLIs (Claude Code,
   Codex, Grok) as backends; we are not competing with them on
   editor polish or model quality.

2. **We are not a Jira / Linear import.** Feedback in Evolvo is not a ticket
   waiting for a human; it's a job that flows through a reviewable pipeline.
   The PM and the engineer are the same person, often the same click.

3. **We are not auto-promote.** Every iteration that lands in your app is one
   you approved. The agent never writes to your source without a human gate.
   This is the safety property the category lacks.

## Safety story (two sentences)

Evolvo runs locally by default — no cloud, no telemetry, no phone-home unless
you opt in. Every iteration the agent produces is gated by an explicit human
approval before it touches your app, so the lineage you ship is one you
reviewed.

The "unless you opt in" hedge is load-bearing. We are not monetizing Evolvo,
but we may host a future opt-in hub where users share their lineages
("share your vibe") with the community. That hub will be opt-in,
transport-only, and never default-on; the local-first guarantee survives.

## Tone & voice

- **Plain English over jargon.** "Provenance-native" is the one piece of
  vocabulary we ask the reader to learn; everything else is conversational.
- **Concrete over aspirational.** "An agent turns feedback into an iteration"
  beats "AI-powered iteration synthesis."
- **Confident, not bombastic.** No "ultimate," no "revolutionary," no "10x."
  We are claiming a category, not selling a supplement.
- **Safety language is normal English.** "Nothing leaves your machine" beats
  "zero-trust local sandbox."
- **The lineage is singular and tangible.** "The lineage tree," "the
  iteration," "the fork" — never "lineage data" or "iteration objects." A
  thing the user can see and click, not a schema.
- **No emojis in formal docs** (README, SECURITY, INSTALL, CONCEPTS).
  Marketing surface (landing page, social posts) may use them sparingly.
- **Don't say "self-evolving" in public copy.** Internally accurate;
  externally it triggers the AGI-safety reflex and costs us two paragraphs
  walking it back. Use "iterative," "generational," or "provenance-native"
  instead. Save "self-evolving" for the architect doc.
- **"Vibe" is a user-facing noun.** A user's lineage is their *vibe*. They
  *share their vibe*, *fork someone else's vibe*, *remix a vibe*. The
  vocabulary is intentionally informal — it makes the formal claim
  ("provenance-native") palatable to the audience we're parasitizing.

## What this brief does not cover

- Logo / wordmark / visual identity.
- Pricing / commercial story (we are open-sourcing, not monetizing).
- Comparison matrix vs every competitor (one paragraph max if needed; see
  the "we are not" claims above).
