# http_json — generic REST helper

Thin `reqwest` wrapper so Leptos code can hit arbitrary JSON APIs through the Tauri host (no CORS, no fetch quirks). Optional bearer-token storage via keyring.

## What you get

- `host.rs` — `http_request(method, url, headers, bearer_ref, body)` → `{ status, body_json }`. `bearer_ref` is an opaque keyring account name so the UI never sees the token.
- Helpers: `http_set_bearer(ref, token)`, `http_has_bearer(ref)`, `http_clear_bearer(ref)`.
- `ui.rs` — typed `fetch_json` wrapper.

## Wire-up

1. Deps: `reqwest` (rustls + json), `keyring`.
2. Copy `host.rs` to `app/src-tauri/src/integrations/http_json.rs`, register in `invoke_handler`.
3. Copy `ui.rs` to `app/ui/src/integrations/http_json.rs`.

## Secret model

- The UI passes a `bearer_ref` (e.g. `"stripe_live"`, `"github_pat"`) instead of the token itself. The host looks up the token in the keyring under service `evolvo.http` / account `<bearer_ref>`. Keeps API keys out of WASM memory / logs.
- For unauthenticated calls, pass `None` / empty string.

## Security notes

- No URL allowlist by default. If you expose this to untrusted user input, add one — the command runs with host privileges and can reach internal addresses.
- Response size is unbounded. Add a byte cap if you're going to call APIs that can return large payloads.
