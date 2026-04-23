# Frontend conventions — Leptos 0.8 CSR + Trunk

## Components

- Prefer function components `#[component] fn Foo(...) -> impl IntoView`.
- Signals: `RwSignal::new(...)` for local state, `Signal`/`ReadSignal` in component props. No global mutable statics.
- Effects: `Effect::new(|_| ...)` — keep side effects (DOM measuring, `web-sys` calls) inside effects, not inside `view!`.
- `view!` macro returns an `impl IntoView`. Prefer many small components over one mega-view. `canvas.rs` is the current exception — intentionally one file because the drawing state is deeply coupled.

## File boundaries

- `main.rs` — mount only.
- `shell.rs` — **invariant** chrome: app bar, Lineage nav/page, Star Us link, Feedback FAB, Canvas overlay + feedback panel composition. Owns `panel_open` and exposes `PanelOpen` via context. Do not re-implement any of this inside `app.rs`.
- `app.rs` — the **NewApp content area**. Mounts `<Shell>` and renders the current app's Home content inside it. Iteration authors rewrite this file; they must not remove `<Shell>` or move chrome into it.
- `canvas.rs` — all drawing / annotation state. Rendering uses `CanvasRenderingContext2d` via `web-sys`.
- `toolbar.rs`, `feedback_panel.rs`, `voice.rs` — one panel/feature per file.
- `interop.rs` — **only** place that talks to Tauri. Keep other modules Tauri-agnostic; pass closures/signals in.
- `types.rs` — UI mirrors of wire types. Keep field names camelCase in the `#[serde]` layer to match the Rust host.

## Interop (`interop.rs`)

- One `async fn` per Tauri command. Signatures take typed inputs, return `Result<T, String>`.
- Use `serde_wasm_bindgen` for args, `JsFuture` to await `invoke`, and `serde_wasm_bindgen::from_value` for the result.
- Errors coming back from Tauri are `JsValue` strings — normalise to `Err(String)` so callers can render them directly.

## web-sys usage

- Every `web-sys` type you use must be listed in `Cargo.toml`'s `features = [...]`. If you add `HtmlCanvasElement::get_context_with_context_options`, add the feature *and* the method's enabling feature.
- Cast via `dyn_into::<HtmlCanvasElement>()` — never `unwrap_throw` in a path the user can hit. Match the `Result`.
- MediaRecorder / clipboard / canvas APIs are browser-gated — on Tauri WebView they're available but behave like Safari/WebKit. Test pasted images + voice capture in the actual Tauri shell before shipping.

## Build

- `trunk serve` (dev) / `trunk build` (release) — configured by `app/ui/Trunk.toml`. Output goes to `app/ui/dist/`, which Tauri reads as `frontendDist`.
- Tauri spawns Trunk via `scripts/trunk-dev.sh` / `trunk-build.sh`. Don't bypass the scripts in CI — they are the contract.
- Release WASM is size-sensitive. Before shipping a new dependency, measure `dist/*.wasm` with and without.

## i18n

- There is no i18n layer today. Strings are English-only. If/when i18n lands, do it in one pass across `app.rs` / `toolbar.rs` / `feedback_panel.rs` / `voice.rs` — do not introduce a half-translated state.

## Accessibility

- Canvas-first app, but every button/control must still have a discoverable label. Add `aria-label` to icon-only buttons. Keyboard shortcuts belong in `toolbar.rs`.
