# Evolvo — Design Guide

Living style guide for the **current** app (host Evolvo, or whatever the
latest NewApp iteration replaced it with). Iterations update this file in
the same change that touches CSS — it is design *memory*, not aspiration.
Read it before `frontend_plan` / `frontend_impl` so each iteration evolves
the existing visual language instead of reinventing it from zero.

## Token vocabulary (`app/ui/styles/tokens.css`)

Names are protected (`validators::REQUIRED_TOKEN_NAMES`); values are the
creative dial. Current values-of-record:

| Group | Tokens | Current values |
|---|---|---|
| Surfaces | `--bg`, `--fg`, `--muted`, `--border`, `--surface`, `--surface-raised`, `--surface-sunken`, `--fg-strong` | `--bg:#fff` `--fg:#0f172a` `--muted:#64748b` `--border:#e2e8f0` `--surface:#fff` `--surface-raised:#fff` `--surface-sunken:#f8fafc` |
| Accent / semantic | `--accent`, `--accent-fg`, `--accent-hover`, `--danger`, `--success` | `--accent:#2563eb` (blue) `--accent-fg:#fff` `--accent-hover:#1d4ed8` `--danger:#dc2626` `--success:#16a34a` |
| Panel | `--panel-bg`, `--panel-shadow` | `rgba(255,255,255,.97)`, soft 40px shadow |
| Radii | `--radius`, `--radius-sm`, `--radius-full` | `10px`, `6px`, `999px` |
| Spacing scale | `--space-1`…`--space-8` | `0.25rem` → `4rem`, roughly 1.25–1.5× steps |
| Type scale | `--text-xs`…`--text-2xl` | `0.75rem` → `1.75rem` |
| Weight | `--weight-normal|medium|semibold|bold` | `400/500/600/700` |
| Elevation | `--shadow-1|2|3` | 1px hairline → 20px soft → 40px soft, all `rgba(15,23,42,…)` |
| Motion | `--transition-fast`, `--transition-base` | `120ms ease`, `200ms ease` |
| Misc | `--tool-size` | `36px` (canvas toolbar buttons) |

One accent color today: `--accent` (blue, `#2563eb`). No secondary/tertiary
accent exists — if an iteration needs a second semantic color, add a new
token name to `tokens.css` (protected file, but additive changes are fine)
rather than hard-coding a value.

## Layout pattern (current Home content, `app/ui/src/app.rs`)

**Pattern: `stack`.** The stock Home page (`HomePage` in `app.rs`) is a
single centered column: full-height flex container
(`.home-page { flex:1; display:flex; align-items:center;
justify-content:center; }`) holding one `.home-hero` block (max-width
560px, text-centered) with a title, a subtitle paragraph, and a left-aligned
tips list. No sidebar, no grid, no multi-panel dashboard — this is
intentionally the simplest possible shape since it exists only to explain
the Feedback FAB before the first NewApp iteration replaces it.

**Density: comfortable.** Spacing uses `--space-5`/`--space-6` between
major blocks, `--text-base`/`--text-2xl` for body/heading — no compact
mode exists yet.

The invariant shell around this content (`app/ui/src/shell.rs`,
`app/ui/styles.css`) is its own layout: a top app bar, a slide-over Lineage
page, a floating Feedback FAB (bottom-right), and the Canvas + Feedback
overlay pair. That chrome is protected and out of scope for iteration
redesigns — see `.claude/rules/common/product-invariants.md`.

## base.css primitive inventory (`app/ui/styles/base.css`)

Protected, append-only. Every primitive already carries `:hover` +
`:focus-visible` states built from tokens — use these before inventing a
new class in `app.css`:

- **Buttons**: `.btn` (default/bordered), `.btn-primary` (accent-filled),
  `.btn-danger` (danger-filled), `.btn-ghost` (borderless). All share
  padding/radius/transition from `.btn`; the modifiers only change
  color/border.
- **Surface**: `.card` — raised surface, 1px border, `--shadow-1` resting /
  `--shadow-2` on hover.
- **Forms**: `.input` (bordered field, focus ring), `.field` (label+input
  flex column), `.field-label` (small muted caption), `.field-hint` (small
  muted helper text below a field).
- **States**: `.empty-state` (centered muted placeholder copy, generous
  vertical padding).
- **Badges**: `.badge` (neutral pill), `.badge-accent` / `.badge-success` /
  `.badge-danger` (tinted via `color-mix(in srgb, var(--token) N%,
  transparent)` — this is the house pattern for tinted backgrounds, use it
  instead of a raw `rgba()`).

## app.css today (`app/ui/styles/app.css`)

Iteration-owned, currently minimal: just the Home/landing rules
(`.home-page`, `.home-hero`, `.home-title`, `.home-subtitle`, `.home-tips`)
styling the stock welcome screen described above. This is the file every
`frontend_impl` stage rewrites; expect it to be replaced wholesale by the
first real NewApp iteration.

## Design decisions

Dated log of intentional visual choices. Append an entry whenever an
iteration changes the layout pattern, reassigns a token value, or makes a
deliberate call on ambiguous canvas input — this is what lets iteration
N+1 understand *why* the current design looks the way it does instead of
guessing from the diff.

- **2026-07-17** — Initial DESIGN.md authored (design-intent-in-plan-pipeline
  work). Documents the stock host Evolvo Home page as the baseline: single
  centered `stack` layout, comfortable density, one accent color (`#2563eb`
  blue), no theme overrides yet applied. No prior design decisions existed
  to log — this file starts empty of history and accumulates from here.
