---
description: Measure the Evolvo WASM bundle size — run trunk build and report dist/*.wasm sizes with a breakdown of top symbols if twiggy is available.
---

Run a release WASM build and report size:

```bash
cd app/ui && trunk build --release
ls -lh dist/
```

If `twiggy` is available on PATH, also run:
```bash
twiggy top -n 20 dist/*.wasm
```

If `cargo-bloat` is available, also run:
```bash
cargo bloat --release --target wasm32-unknown-unknown -p evolvo_ui -n 20
```

Report:
- Total `dist/` size
- Per-file sizes (especially `*.wasm`)
- Top 20 symbols by size (if tools available)
- One sentence assessing whether this is within budget (soft cap: a few MB for the wasm file).

Do NOT make changes — this is a measurement command.
