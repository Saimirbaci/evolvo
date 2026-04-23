# webhook_ingest — loopback HTTP receiver

Run a tiny HTTP server inside the Tauri host so the NewApp can receive webhook callbacks from services that need to POST back (OAuth redirects, Stripe events in dev, GitHub webhooks tunneled in, …).

## What you get

- `host.rs` — `webhook_start(path_prefix)` → returns `http://127.0.0.1:<port>/<path_prefix>/…`; `webhook_stop()`; `webhook_drain()` returns the queued events as `Vec<WebhookEvent>`.
- Events are kept in a bounded in-memory queue (default 500). Older events are dropped on overflow — this is a dev helper, not a production queue.

## Wire-up

1. Deps: `axum`, `tokio`, `tower` (for service). Or swap for `warp` / `hyper` — any async HTTP lib works.
2. Copy `host.rs` to `app/src-tauri/src/integrations/webhook_ingest.rs`, register the three commands.

## Security

- Binds `127.0.0.1` only. Never `0.0.0.0`.
- No auth on the endpoint. If the receiver needs to validate a signing secret (Stripe, GitHub), do it inside the handler before enqueueing.
- Port is picked dynamically and returned to the UI — never hardcode.

## Typical flow

1. UI calls `webhook_start("stripe")` → gets a URL like `http://127.0.0.1:52199/stripe`.
2. User pastes that URL (via an `ngrok` tunnel etc.) into the third-party service's webhook config.
3. UI polls `webhook_drain()` every few seconds (or subscribe to a Tauri event from the handler) to process received payloads.
