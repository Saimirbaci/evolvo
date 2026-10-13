# timeline_feed

A chronological event log. Each event has a `kind`, a `summary` line, an
optional structured `payload`, and two timestamps (`occurredAt` —
when the event happened in the source domain; `ingestedAt` — when the
local app saw it). Filters by kind, date range, and free-text.

## What you get

- One entity: `Event { id, kind, summary, payload, occurredAt, ingestedAt }`.
- Four Tauri commands: `list_events` (with optional filter args),
  `load_event`, `record_event`, `delete_event`.
- One trait: `EventStore` (object-safe). Default impl `JsonEventStore`
  persists `<workspace>/timeline_feed/events/{id}.json`.
- One Leptos component: `TimelineFeedPage`. Filter row + chronological
  list with day-grouped headers.
- ~80 lines of CSS.

## Mount points

| What             | Where                                                   |
|------------------|---------------------------------------------------------|
| Host commands    | `app/src-tauri/src/app_shapes/timeline_feed.rs`         |
| invoke_handler!  | `app/src-tauri/src/lib.rs`                              |
| UI page          | `app/ui/src/app_shape_timeline_feed.rs`                 |
| Styles (append)  | `app/ui/styles.css`                                     |

## Data interface

```
trait EventStore: Send + Sync {
    fn list(&self) -> Result<Vec<Event>, String>;
    fn load(&self, id: &str) -> Result<Event, String>;
    fn save(&self, event: &Event) -> Result<(), String>;
    fn delete(&self, id: &str) -> Result<bool, String>;
}
```

`list_events` filters in Rust over `EventStore::list()` so the trait
stays simple — sqlite seams should override `list` to push filters into
SQL.

## Backend seams

### `webhook_ingest` (the headline pairing)

When this binding is set, the implementer mounts the webhook_ingest
integration's loopback server inside the app's startup and routes
incoming POSTs into `record_event`. Sketch:

```rust
// startup.rs
let store = state.store().clone();
crate::integrations::webhook_ingest::start_server(move |req| {
    let kind = req.path.split('/').last().unwrap_or("event").to_string();
    let payload: Json = req.json_body().unwrap_or(Json::Null);
    let summary = payload.get("summary")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("{kind} event"));
    let now = current_time_unix_ms();
    let occurred_at = payload.get("occurredAtUnixMs")
        .and_then(|v| v.as_u64())
        .unwrap_or(now);
    let event = Event { id: format!("ev-{now}"), kind, summary, payload, occurred_at, ingested_at: now };
    JsonEventStore::new(&store).save(&event)?;
    Ok(())
}).await?;
```

### `sqlite`

Replace with `SqliteEventStore`:

```sql
CREATE TABLE IF NOT EXISTS events (
  id           TEXT PRIMARY KEY,
  kind         TEXT NOT NULL,
  summary      TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  occurred_at  INTEGER NOT NULL,
  ingested_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS events_by_occurred ON events(occurred_at DESC);
CREATE INDEX IF NOT EXISTS events_by_kind ON events(kind);
```

`list_events` should accept `since` / `until` / `kinds[]` / `q` arg
fields and translate them to SQL — a thousand-event feed becomes a
hundred-event slice.

### `http_json`

Pull a remote events endpoint on schedule. The implementer writes a
`tokio::time::interval` task that GETs `<base>/events?since=<last>` and
calls `record_event` for each new entry. Last-seen timestamp lives in
`<workspace>/timeline_feed/_cursor.json`.

## FILL markers

| Marker                                | What to fill                                                                |
|---------------------------------------|-----------------------------------------------------------------------------|
| `FILL: event kind palette`            | The list of `kind` values the user's domain produces (e.g. "deploy", "alert", "ticket"). Used by the kind filter `<select>`. |
| `FILL: live tail polling` (opt)       | A repeating client-side reload (every N seconds) so new events appear without a manual refresh. |
| `FILL: per-kind icon` (opt)           | Icon mapping in the row renderer for each event kind.                        |
| `FILL: payload preview rendering` (opt) | A nicer payload view than the default `<pre>{json}</pre>`.                  |

## Mounting inside the Shell

```rust
#[component]
pub fn App() -> impl IntoView {
    view! { <Shell><TimelineFeedPage/></Shell> }
}
```

## Verify

```bash
cargo check --workspace
cargo check -p evolvo_ui --target wasm32-unknown-unknown
cargo test  -p evolvo_desktop --lib
```
