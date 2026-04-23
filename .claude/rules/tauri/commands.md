# Tauri 2 command conventions — Evolvo

## Adding a new command

A new Tauri command requires changes in **three** places, or it will silently 404 at call time:

1. **Define** in `app/src-tauri/src/commands.rs` with `#[tauri::command]`, returning `Result<T, String>`.
2. **Register** in the `invoke_handler` inside `app/src-tauri/src/lib.rs` (or `main.rs` if that's where `generate_handler!` lives — check first).
3. **Wrap** in `app/ui/src/interop.rs` with a typed `async fn` that calls `invoke("command_name", args)` and deserialises the result.

If you add a command but skip step 2, the UI gets a runtime error like `command foo_bar not found` — this is the single most common regression. Grep for the old command names when adding a new one to confirm you've updated every site.

## Payload shape

- One struct per command (or reuse `EntityIdPayload` for single-id commands). The struct is `#[serde(rename_all = "camelCase")]`.
- Keep binary inputs (images, audio) as base64 strings in the payload, decoded inside the command via `STANDARD.decode`.
- Return a domain record, not a DTO — the UI re-derives its view types in `ui/src/types.rs`.

## State

- Shared state uses `tauri::State<'_, AppState>`. `AppState` owns the `Store` and knows the workspace root.
- State is constructed in `main.rs` / `lib.rs::run()` and passed in via `.manage(...)`. Don't construct a `Store` ad-hoc inside a command — go through state so tests can override the root.

## Security

- Commands run with full host privileges. Treat every payload as untrusted:
  - Filenames → `sanitise_filename`.
  - Base64 → size-bounded decode (add a cap if payloads grow).
  - Never use `payload.feedback_id` as a raw path component — always via `Store` helpers that scope to `attachments/{id}/`.
- The `csp` in `tauri.conf.json` is currently `null` (dev convenience). If shipping a bundled build, tighten CSP before release.

## Capabilities / `capabilities/`

- Start with the minimum capability set. Don't enable `shell:allow-open` or filesystem capabilities unless a command actually needs them — the local `Store` does not.

## Testing commands

- The `#[tauri::command]` macro's generated shim is hard to invoke from a unit test. Test the **body** by extracting logic into a plain function (`submit_feedback_impl(&Store, payload)`) and calling that from tests — see the existing pattern. Keep the `#[tauri::command]` wrapper thin (≤ ~15 lines).
