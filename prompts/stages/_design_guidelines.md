# Design guidelines

Read `DESIGN.md` at the worktree root before you plan or touch CSS.
**Evolve this design, don't reinvent it.** Deviate only where the user's
feedback or drawing explicitly asks you to. Record every deviation:
theme/palette changes go in `plan.frontend.design.themeOverrides`
(with a `reason`), interpretations of ambiguous marks go in
`plan.frontend.design.visualNotes`. Update `DESIGN.md`'s
`## Design decisions` log in the same change whenever you make a call
that future iterations need to know about.

## Baseline quality bar

- Use the spacing scale (`--space-1`…`--space-8`) and type scale
  (`--text-xs`…`--text-2xl`) from `tokens.css` — no ad-hoc `px` values for
  rhythm or font sizing.
- **One accent color per app.** Don't introduce a second competing brand
  color; if the app genuinely needs a second semantic color (e.g. a
  warning tier), add a new named token to `tokens.css` rather than a raw
  literal in `app.css`.
- Every interactive element (button, link, input, row-with-action) needs
  visible `:hover`, `:focus-visible`, and — where applicable — `:disabled`
  states. A control that only looks different on click is not enough.
- Empty, loading, and error states are **designed**, not defaulted. A bare
  "No data" or a silently blank panel is a regression. Use `.empty-state`
  as the starting point and give it copy specific to the feature.
- Prefer `app/ui/styles/base.css` primitives (`.btn*`, `.card`, `.input`,
  `.field*`, `.empty-state`, `.badge*`) before inventing a new class in
  `app.css`. Only write new CSS for shapes the primitives genuinely don't
  cover.

## Canvas interpretation rules

The user's drawing is a set of claims, not a spec written in a design
tool — read intent, don't transcribe pixels literally:

- A colored stroke (circle, box, arrow, underline) usually marks
  **emphasis or grouping**, not a literal color request. A red box around
  a button typically means "this needs attention" or "fix this", not
  "make this button red".
- Only treat a mark as a literal color instruction when the context makes
  it unambiguous — the user wrote text like "make this purple" or "change
  the accent to green", or they recolored an already-themed element (e.g.
  scribbled directly over the header bar in a clearly deliberate,
  full-coverage way). In that case express it as a `themeOverrides` entry
  (token + value + reason), **never** as a hard-coded color inside a
  component-specific CSS rule — a literal color belongs to the theme, not
  to one element.
- Every interpretation you make of an ambiguous region — "this red
  circle means X, not Y" — gets one line in `visualNotes` (`region` +
  `interpretation`) so the human reviewer can see your reasoning and
  correct it before implementation runs, not after.
- Genuinely unresolvable ambiguity (you truly cannot tell what the user
  wants) goes in `plan.openQuestions`, not a silent guess buried in
  `visualNotes`.
