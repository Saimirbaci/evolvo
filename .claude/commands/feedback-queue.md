---
description: List pending feedback rows from the local workspace (~/.evolvo/noide_workspace/feedback) and propose the top 5 to work, ranked.
---

Read feedback JSON from `${NOIDE_WORKSPACE_ROOT:-$HOME/.evolvo/noide_workspace}/feedback/`. For each file, extract `id`, `status`, `feedbackType`, `pageRoute`, first 120 chars of `feedbackText`, and `createdAtUnixMs`.

Keep only rows with `status == "new"` or `status == "triaged"`. Rank by: (bug > confusion > improvement > feature_request > compliment), then recency.

Output:
1. A table of the top 10.
2. A short recommendation of which 3–5 to pick up first, clustered by `pageRoute` where possible.
3. Do NOT write to any file. Read-only.

If no workspace exists, say so and stop — don't create one.
