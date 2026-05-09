# data_grid

A sortable, filterable rows-and-columns grid with inline editing.
Schemaless: each row's `cells` is a JSON object, so the agent can declare
the columns the user actually wants without changing the wire types.

## What you get

- One entity: `Row { id, cells, updatedAt }`. `cells` is
  `serde_json::Value` so columns can be added/removed without a schema
  migration.
- Four Tauri commands: `list_rows`, `load_row`, `save_row`, `delete_row`.
- One trait: `RowStore` (object-safe). Default impl `JsonRowStore`
  persists `<workspace>/data_grid/rows/{id}.json`.
- One Leptos component: `DataGridPage`. Filter input on top, column
  headers click-to-sort, cells editable inline.
- ~80 lines of CSS.

## Mount points

| What                  | Where                                                              |
|-----------------------|--------------------------------------------------------------------|
| Host commands         | `app/src-tauri/src/app_shapes/data_grid.rs`                        |
| invoke_handler!       | `app/src-tauri/src/lib.rs`                                         |
| UI page               | `app/ui/src/app_shape_data_grid.rs`                                |
| Styles (append)       | `app/ui/styles.css`                                                |

`app/ui/src/app.rs` keeps mounting `<Shell>`. Replace `HomePage`'s
content with `<DataGridPage/>`.

## Data interface

```
trait RowStore: Send + Sync {
    fn list(&self) -> Result<Vec<Row>, String>;
    fn load(&self, id: &str) -> Result<Row, String>;
    fn save(&self, row: &Row) -> Result<(), String>;
    fn delete(&self, id: &str) -> Result<bool, String>;
}
```

Default impl: `JsonRowStore` (`host.rs`). Files at
`<workspace>/data_grid/rows/{id}.json`.

## Backend seams

### `sqlite`

Replace with `SqliteRowStore`. Schema:

```sql
CREATE TABLE IF NOT EXISTS rows (
  id         TEXT PRIMARY KEY,
  cells_json TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS rows_updated_at ON rows(updated_at DESC);
```

`list` → `SELECT … ORDER BY updated_at DESC`. Sorting by a *cell* column
stays UI-side until the user has enough rows that it matters; in that
case extend the seam to materialise sortable columns into their own
`TEXT GENERATED ALWAYS AS (json_extract(cells_json, '$.<col>')) VIRTUAL`
columns and add a partial index.

### `http_json`

`list` → `GET <base>/rows`; `load` → `GET <base>/rows/{id}`; `save` →
`PUT <base>/rows/{id}`; `delete` → `DELETE <base>/rows/{id}`.

### `file_picker`

When this binding is selected, the implementer wires an "Import CSV"
button into `DataGridPage`'s toolbar that calls the file_picker
integration's `pick_and_parse_csv` (or `pick_and_parse_xlsx`) command,
then batches one `save_row` per parsed row. The first row of the CSV
becomes the column schema; subsequent rows become `Row.cells`. Adapter
file: `templates/integrations/file_picker/seam_data_grid.rs` (write
when the binding is selected).

## FILL markers

| Marker                                 | What to fill                                                                  | Example                                                        |
|----------------------------------------|-------------------------------------------------------------------------------|----------------------------------------------------------------|
| `FILL: column schema`                  | The list of columns + display labels the user's request implies.              | `vec![("name", "Name"), ("email", "Email"), ("status", "Status")]` |
| `FILL: cell editor for non-string columns` | Specialised editors for date / number / enum cells.                          | A `<select>` for a fixed-set status column.                    |
| `FILL: keyboard shortcuts` (opt)       | Tab / shift-tab between cells, ↩ to commit, Esc to cancel.                    | Standard spreadsheet keymap.                                   |
| `FILL: bulk actions` (opt)             | Multi-row select + delete / duplicate / export.                               | Checkbox column + a "Delete selected" toolbar button.          |

## Mounting inside the Shell

```rust
#[component]
pub fn App() -> impl IntoView {
    view! { <Shell><DataGridPage/></Shell> }
}
```

The shell's FAB still opens the Canvas overlay over the grid — the user
can annotate any cell or column header and submit feedback against the
exact view they were looking at.

## Verify

```bash
cargo check --workspace
cargo check -p evolvo_ui --target wasm32-unknown-unknown
cargo test  -p evolvo_desktop --lib
```
