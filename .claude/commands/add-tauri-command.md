---
description: Scaffold a new Tauri 2 command end-to-end — define in commands.rs, register in invoke_handler, wrap in interop.rs. Takes the command name as argument.
argument-hint: <command_name> [payload struct description]
---

Add a new Tauri command named `$ARGUMENTS`. You MUST update all three sites or the UI will 404 at runtime:

1. **Define** `#[tauri::command] pub fn <name>(state: State<'_, AppState>, payload: <Payload>) -> Result<T, String>` in `app/src-tauri/src/commands.rs`. Return a domain record. Use `store_error` for `StoreError` conversion. Validate inputs; sanitise any filename via `sanitise_filename`.
2. **Register** the handler in `app/src-tauri/src/lib.rs` (or wherever `tauri::generate_handler!` is called). Grep first to confirm location.
3. **Wrap** in `app/ui/src/interop.rs` with a typed `async fn` that serialises the payload with `serde_wasm_bindgen::to_value` and deserialises the result.

Also:
- Add a payload struct with `#[serde(rename_all = "camelCase")]` if no existing one fits.
- Add a unit test next to the command body that exercises the underlying logic (not the `#[tauri::command]` shim) using a `tempdir()` workspace.

After the changes, run:
```bash
cargo check --workspace
cargo test -p evolvo_desktop
cargo check -p evolvo_ui --target wasm32-unknown-unknown
```

Confirm all three pass before reporting done.
