# Integration templates

Copy-in modules the NewApp agent can drop into `app/src-tauri/src/integrations/` (host) and `app/ui/src/integrations/` (UI). Each template is self-contained: a `README.md` with wiring instructions, `host.rs` with the Tauri commands, `ui.rs` with the Leptos helper, and `deps.toml` listing the Cargo dependencies to merge into the workspace.

## How the agent uses these

1. Pick the templates the NewApp actually needs — don't copy all of them.
2. Merge each template's `deps.toml` into `Cargo.toml` (`[workspace.dependencies]` when shared) and the relevant crate's `[dependencies]` block.
3. Copy `host.rs` to `app/src-tauri/src/integrations/<name>.rs`, add `pub mod <name>;` to `integrations/mod.rs` (create the module if it doesn't exist), and register the `#[tauri::command]` functions in the `invoke_handler` in `app/src-tauri/src/lib.rs`.
4. Copy `ui.rs` to `app/ui/src/integrations/<name>.rs` and add the typed `invoke` wrappers to `app/ui/src/interop.rs`.
5. Read the template's `README.md` for secrets / first-run setup (some templates need the user to paste an API key into a settings screen or swap a dev `client_id`).

## Available templates

| Template | Purpose | Secret model |
|---|---|---|
| `openrouter/` | Multi-model LLM chat (OpenAI, Anthropic, Gemini, …) with persistent sessions | API key via OS keyring |
| `google_oauth/` | Google Sign-In via OAuth2 PKCE + loopback redirect; unlocks Gmail / Drive / Calendar | Refresh token in keyring; ships a dev `client_id` the user must swap |
| `http_json/` | Thin `reqwest` wrapper for arbitrary REST APIs | Bearer token in keyring (optional) |
| `sqlite/` | Local `rusqlite` DB with migrations | n/a |
| `webhook_ingest/` | Loopback HTTP server for receiving third-party callbacks | n/a |
| `file_picker/` | File open/save dialog + CSV/XLSX import helpers | n/a |

## Important caveats

- **Client secrets in a desktop binary are not secret.** These templates use public-client flows (OAuth PKCE) or user-supplied API keys. Never bake a `client_secret` into a template.
- **Secret storage uses the `keyring` crate** (OS keychain / libsecret / Credential Manager). The user is prompted once per secret. Don't fall back to writing keys to disk.
- **Workspace root** comes from the host `Store`'s layout — persistent template state (chat sessions, tokens, DB file) lives under `<workspace>/integrations/<name>/`.
