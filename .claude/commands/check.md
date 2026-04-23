---
description: Run the full Evolvo build contract — cargo check/test/clippy on host + wasm check on UI. Report pass/fail per gate.
---

Run every gate and report which pass / fail. Do not stop on the first failure — run all gates so the user sees the full picture.

```bash
cargo check --workspace
cargo test -p noide_desktop
cargo clippy -p noide_desktop -- -D warnings
cargo check -p noide_ui --target wasm32-unknown-unknown
```

Summarize in a table: gate | status | first error line (if failed). Do NOT attempt fixes unless the user asks.
