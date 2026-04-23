# OpenRouter LLM integration

Drop-in chat backend that routes through [OpenRouter](https://openrouter.ai) so the NewApp can talk to GPT-4, Claude, Gemini, Llama, etc. through a single API. Keeps a persistent multi-turn session on disk, injects the app name as a system prompt, and stores the API key in the OS keyring.

## What you get

**Host (`app/src-tauri/src/integrations/openrouter/`):**
- `mod.rs` / `host.rs` — Tauri commands: `openrouter_set_api_key`, `openrouter_has_api_key`, `openrouter_clear_api_key`, `openrouter_create_session`, `openrouter_list_sessions`, `openrouter_load_session`, `openrouter_delete_session`, `openrouter_send_message`, `openrouter_list_models`.
- `session.rs` — `ChatSession` + `Message` types, JSON persistence under `<workspace>/integrations/openrouter/sessions/<id>.json`.
- Unit tests (tempdir-based) for session round-trip and the system-prompt builder.

**UI (`app/ui/src/integrations/openrouter/`):**
- `ui.rs` — Typed interop wrappers and a ready-to-mount `<ChatPanel app_name="MyApp"/>` Leptos component with session list, message history, composer, model picker, and an API-key entry modal triggered when `openrouter_has_api_key` returns false.

## Wire-up steps

1. **Deps.** Merge `deps.toml` into `app/src-tauri/Cargo.toml` and `Cargo.toml` (workspace). Key adds: `reqwest` (rustls + json), `keyring`, `tokio` (already present transitively via tauri), `uuid`, `async-trait` (not needed — we use plain `async fn`).

2. **Copy files.** `host.rs` → `app/src-tauri/src/integrations/openrouter/host.rs`; `session.rs` → sibling; `ui.rs` → `app/ui/src/integrations/openrouter.rs`.

3. **Register commands.** In `app/src-tauri/src/lib.rs`, add to the `invoke_handler`:
   ```rust
   crate::integrations::openrouter::host::openrouter_set_api_key,
   crate::integrations::openrouter::host::openrouter_has_api_key,
   crate::integrations::openrouter::host::openrouter_clear_api_key,
   crate::integrations::openrouter::host::openrouter_create_session,
   crate::integrations::openrouter::host::openrouter_list_sessions,
   crate::integrations::openrouter::host::openrouter_load_session,
   crate::integrations::openrouter::host::openrouter_delete_session,
   crate::integrations::openrouter::host::openrouter_send_message,
   crate::integrations::openrouter::host::openrouter_list_models,
   ```

4. **Mirror in interop.** Add the typed `async fn` wrappers from `ui.rs`'s `interop` section into `app/ui/src/interop.rs` (or keep them co-located in the integration module — either works).

5. **Mount the UI.** In the NewApp page where you want chat, render `<ChatPanel app_name=\"<your app>\"/>`. The `app_name` prop becomes the first sentence of the system prompt so the LLM knows which app it's helping the user build.

## Secrets

- **API key**: stored via the `keyring` crate under service `evolvo.openrouter`, account `api_key`. First use shows a modal asking the user to paste their OpenRouter key (sign up at openrouter.ai; free tier works).
- **No fallback to disk**: if the keyring is unavailable, the commands return an error and the UI surfaces it.
- **Clearing**: the UI's settings dropdown calls `openrouter_clear_api_key`.

## Session persistence

- One JSON file per session at `<workspace>/integrations/openrouter/sessions/<id>.json`.
- Each file holds: `id`, `app_name`, `model`, `created_at_ms`, `updated_at_ms`, `messages: Vec<Message>`.
- Sessions are enumerated by directory scan (cheap for <~1k sessions; swap for an index file if you exceed that).
- The system prompt is rebuilt on every send from `app_name` + optional per-session extras — never stored in the message history, so swapping the app name later just works.

## Default model

`openrouter_create_session` defaults to `openai/gpt-4o-mini` (cheap, fast, decent). The UI's model picker lets the user switch; `openrouter_list_models` calls OpenRouter's `/models` endpoint to populate it.

## Streaming

Non-streaming `/chat/completions` for MVP simplicity. To upgrade to streaming, swap `.send().await?.json()` for an SSE reader and emit Tauri events (`window.emit("openrouter.chunk", ...)`) the UI subscribes to with `listen`. The session file is written once when the stream closes.

## Error surface

All commands return `Result<T, String>`. Network / 4xx / 5xx errors are stringified at the boundary; the UI renders them in a dismissible banner.
