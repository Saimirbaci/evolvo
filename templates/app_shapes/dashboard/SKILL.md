# dashboard

A grid of widget tiles. Each widget has a kind (number / text / chart),
an optional `sourceUrl`, an optional `refreshIntervalMs`, and the latest
fetched value cached inline. The user can add / edit / delete widgets;
each widget can be refreshed on demand or on a timer.

## What you get

- One entity: `Widget { id, title, kind, sourceUrl, refreshIntervalMs,
  ordinal, config, latestValue, latestFetchedAt }`. `kind` is a free
  string ("number", "text", "bar_chart", …) so additional kinds slot in
  via the `// FILL: widget kinds` marker without changing the wire type.
- Five Tauri commands: the standard four CRUD + `refresh_widget` which
  re-fetches a single widget's `sourceUrl` and updates `latestValue`.
- One trait: `DashboardStore` (object-safe). Default impl
  `JsonDashboardStore` persists `<workspace>/dashboard/widgets/{id}.json`.
- One Leptos component: `DashboardPage` (in `ui_app.rs`). Toolbar with
  "Add widget" + a CSS grid of tiles.
- ~80 lines of CSS.

## Mount points

| What             | Where                                                   |
|------------------|---------------------------------------------------------|
| Host commands    | `app/src-tauri/src/app_shapes/dashboard.rs`             |
| invoke_handler!  | `app/src-tauri/src/lib.rs`                              |
| UI page          | `app/ui/src/app_shape_dashboard.rs`                     |
| Styles (append)  | `app/ui/styles.css`                                     |

## Data interface

```
trait DashboardStore: Send + Sync {
    fn list(&self) -> Result<Vec<Widget>, String>;
    fn load(&self, id: &str) -> Result<Widget, String>;
    fn save(&self, widget: &Widget) -> Result<(), String>;
    fn delete(&self, id: &str) -> Result<bool, String>;
}
```

`refresh_widget` is its own Tauri command (not on the trait) so the
fetch-and-update sequence stays in one place. The default scaffold
implements it as a no-op that just stamps `latestFetchedAt` — replace
the body when bound to `http_json`.

## Backend seams

### `http_json` (the headline pairing)

Replace the `// FILL: refresh implementation` marker in `host.rs` with:

```rust
let raw = crate::integrations::http_json::get_json(&state, &widget.source_url?).await?;
widget.latest_value = Some(raw);
widget.latest_fetched_at = Some(current_time_unix_ms());
```

The response can be transformed by per-widget config (e.g. a JSONPath
expression in `widget.config.path`) — that lives in the same FILL
marker.

### `sqlite`

Add:

```sql
CREATE TABLE IF NOT EXISTS widget_readings (
  widget_id  TEXT NOT NULL,
  value_json TEXT NOT NULL,
  fetched_at INTEGER NOT NULL,
  PRIMARY KEY(widget_id, fetched_at)
);
```

When `refresh_widget` succeeds, INSERT into `widget_readings` in
addition to updating the inline `latestValue`. A new
`load_widget_history(id, since, limit)` command (add to lib.rs's
invoke_handler!) returns the readings the chart kinds need.

## FILL markers

| Marker                                     | What to fill                                                                                  |
|--------------------------------------------|-----------------------------------------------------------------------------------------------|
| `FILL: widget kinds`                       | Render-side switch on `widget.kind`. Default ships `number` and `text`; extend for chart kinds. |
| `FILL: refresh implementation`             | The body of `refresh_widget`. Default is a no-op stamp; replace per the http_json seam.       |
| `FILL: layout customisation` (opt)         | Drag-to-reorder, custom column counts, per-widget size hints.                                 |
| `FILL: chart rendering` (opt)              | A real chart component for `kind == "bar_chart"` / `"line_chart"`. Defer until needed.        |

## Mounting inside the Shell

```rust
#[component]
pub fn App() -> impl IntoView {
    view! { <Shell><DashboardPage/></Shell> }
}
```

## Verify

```bash
cargo check --workspace
cargo check -p evolvo_ui --target wasm32-unknown-unknown
cargo test  -p evolvo_desktop --lib
```
