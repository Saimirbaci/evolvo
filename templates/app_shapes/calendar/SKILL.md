# calendar

A month-grid calendar. Each cell is a date; clicking a cell opens that
day's event list and a "+ Event" button. Events have a title, start/end
timestamps, an `all_day` flag, and optional notes.

## What you get

- One entity: `CalendarEvent { id, title, startMs, endMs, allDay, notes }`.
- Four Tauri commands: `list_events_in_range` (start, end), `load_event`,
  `save_event`, `delete_event`.
- One trait: `CalendarStore` (object-safe). Default impl
  `JsonCalendarStore` persists `<workspace>/calendar/events/{id}.json`.
- One Leptos component: `CalendarPage`. Header with prev/next month +
  "today" button. 7-column × 6-row month grid below.
- ~90 lines of CSS.

## Mount points

| What             | Where                                                   |
|------------------|---------------------------------------------------------|
| Host commands    | `app/src-tauri/src/app_shapes/calendar.rs`              |
| invoke_handler!  | `app/src-tauri/src/lib.rs`                              |
| UI page          | `app/ui/src/app_shape_calendar.rs`                      |
| Styles (append)  | `app/ui/styles.css`                                     |

## Data interface

```
trait CalendarStore: Send + Sync {
    fn list_in_range(&self, start_ms: u64, end_ms: u64) -> Result<Vec<CalendarEvent>, String>;
    fn load(&self, id: &str) -> Result<CalendarEvent, String>;
    fn save(&self, event: &CalendarEvent) -> Result<(), String>;
    fn delete(&self, id: &str) -> Result<bool, String>;
}
```

`list_in_range` is on the trait so the sqlite seam can push the date
filter into SQL. The default JSON impl scans the directory and filters
in memory — fine up to a few hundred events.

## Backend seams

### `google_oauth` (the headline pairing)

Replace `JsonCalendarStore` with `GoogleCalendarStore`. The integration
already manages the OAuth refresh token in the OS keyring; the seam
wraps the Calendar v3 REST API:

- `list_in_range` → `events.list` with `timeMin` / `timeMax`.
- `load` → `events.get`.
- `save` → `events.insert` for new ids, `events.patch` for existing.
- `delete` → `events.delete`.

Map `CalendarEvent.allDay = true` to Google's `start.date` / `end.date`
form; otherwise use `start.dateTime` / `end.dateTime`. Persist a
`<workspace>/calendar/_sync.json` cursor so incremental syncs use
Google's `syncToken`.

The seam needs the `https://www.googleapis.com/auth/calendar.events`
scope; the implementer adds it to the google_oauth integration's
configured scope list during BackendImpl.

### `sqlite`

Schema:

```sql
CREATE TABLE IF NOT EXISTS events (
  id       TEXT PRIMARY KEY,
  title    TEXT NOT NULL,
  start_ms INTEGER NOT NULL,
  end_ms   INTEGER NOT NULL,
  all_day  INTEGER NOT NULL,
  notes    TEXT
);
CREATE INDEX IF NOT EXISTS events_by_start ON events(start_ms);
```

`list_in_range` becomes `SELECT … WHERE start_ms < ? AND end_ms > ?
ORDER BY start_ms`. Worth doing past ~200 events.

### `http_json`

Out-of-scope for the prototype. The manifest entry reserves the seam
slot; a future iteration could implement CalDAV-over-HTTP here.

## FILL markers

| Marker                              | What to fill                                                                                  |
|-------------------------------------|-----------------------------------------------------------------------------------------------|
| `FILL: event categories`            | Add a `category: Option<String>` field to `CalendarEvent` and a per-category color in CSS.    |
| `FILL: week view` (opt)             | A 7-day horizontal hour grid. Switch tabs with the existing month view.                        |
| `FILL: agenda view` (opt)           | Flat chronological list of upcoming events. Useful complement to the month grid.               |
| `FILL: timezone handling` (opt)     | Today the scaffold assumes the user's local timezone. Surface a tz picker if the audience is mixed-tz. |

## Mounting inside the Shell

```rust
#[component]
pub fn App() -> impl IntoView {
    view! { <Shell><CalendarPage/></Shell> }
}
```

## Verify

```bash
cargo check --workspace
cargo check -p evolvo_ui --target wasm32-unknown-unknown
cargo test  -p evolvo_desktop --lib
```
