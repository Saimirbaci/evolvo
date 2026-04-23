# Google OAuth integration

OAuth 2.0 with PKCE + loopback redirect so a NewApp can sign the user into their Google account and call Gmail / Drive / Calendar APIs. Refresh token goes into the OS keyring.

## What you get

- `host.rs` — Tauri commands: `google_begin_sign_in`, `google_complete_sign_in`, `google_sign_out`, `google_is_signed_in`, `google_access_token`, `google_fetch_userinfo`.
- `ui.rs` — `<GoogleSignInButton/>` Leptos component + interop wrappers.
- Loopback redirect via `tauri-plugin-oauth` (starts an ephemeral HTTP server on `127.0.0.1:<random>`).

## Secrets

- The template ships a placeholder `GOOGLE_CLIENT_ID` constant — **the user must swap it for their own OAuth 2.0 Web/Desktop client ID**. Create one at <https://console.cloud.google.com/apis/credentials>, authorized redirect URI `http://127.0.0.1` (loopback — port is dynamic).
- No `client_secret`. Google's public-client OAuth2 PKCE flow doesn't need one; baking secrets into a desktop binary is pointless anyway.
- Refresh token is stored in the OS keyring under `evolvo.google` / `refresh_token`. Access tokens are re-minted from it on demand (short-lived, kept in memory only).

## Wire-up

1. `deps.toml` → workspace / crate `Cargo.toml`.
2. Copy `host.rs` and `ui.rs` into `app/src-tauri/src/integrations/google_oauth/` and `app/ui/src/integrations/google_oauth.rs`.
3. Register the Tauri plugin in `lib.rs`:
   ```rust
   tauri::Builder::default()
       .plugin(tauri_plugin_oauth::init())
       // ...
   ```
4. Register the eight commands in the `invoke_handler`.
5. Paste your Google OAuth client ID into `GOOGLE_CLIENT_ID` in `host.rs` (or load it from a build-time env with `option_env!`).
6. Requested scopes default to `openid email profile`. Add Gmail/Drive/Calendar scopes to the `SCOPES` const as needed.

## Flow

1. User clicks the button → `google_begin_sign_in` starts a loopback server, returns an auth URL.
2. UI opens the URL in the system browser.
3. User approves → Google redirects to `http://127.0.0.1:<port>/?code=...`.
4. The plugin captures the code, UI calls `google_complete_sign_in(code)` → host exchanges for tokens, stores the refresh token in keyring.
5. Subsequent `google_access_token()` calls return a fresh access token minted from the refresh token.

## Using with Gmail / Drive / Calendar

Pair this with `http_json` — call `google_access_token()`, pass the returned bearer to the HTTP client. Or extend this module with typed wrappers (`gmail_list_messages`, `drive_upload_file`, etc.) once you know which APIs the NewApp actually needs.
