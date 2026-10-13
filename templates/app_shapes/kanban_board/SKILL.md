# kanban_board

A three-column board (Todo / Doing / Done) where the user creates cards
and drags them between columns. The whole board is persisted as one
document; cards are an inline collection inside it. CRUD goes through a
`BoardStore` trait so the persistence layer can be swapped.

## What you get

- Two entities: `BoardState { version, columns, updatedAt }` and
  `Card { id, title, body, columnId }`.
- Four Tauri commands: `list_boards` (always returns a single-element
  list — the singleton board), `load_board`, `save_board`, `reset_board`.
- One trait: `BoardStore` (object-safe). Default impl `JsonBoardStore`
  persists `<workspace>/kanban_board/board.json`.
- One Leptos component: `KanbanBoardPage` (in `ui_app.rs`). HTML5
  drag-and-drop wires the move-between-columns interaction.
- ~70 lines of CSS in `styles.css` (3-column layout, card chrome).

## Mount points

| What                  | Where                                                              |
|-----------------------|--------------------------------------------------------------------|
| Host commands         | `app/src-tauri/src/app_shapes/kanban_board.rs`                     |
| invoke_handler!       | `app/src-tauri/src/lib.rs`                                         |
| UI page               | `app/ui/src/app_shape_kanban_board.rs`                             |
| Styles (append)       | `app/ui/styles.css`                                                |

`app/ui/src/app.rs` keeps mounting `<Shell>`. Replace `HomePage`'s
content with `<KanbanBoardPage/>`.

## Data interface

```
trait BoardStore: Send + Sync {
    fn load(&self) -> Result<BoardState, String>;
    fn save(&self, board: &BoardState) -> Result<(), String>;
    fn reset(&self) -> Result<(), String>;
}
```

`list` returns `vec![load()?]` for symmetry with the standard CRUD
shape (planners that build dashboards "across boards" find a list-y API
familiar). The default JSON impl persists
`<workspace>/kanban_board/board.json`.

## Backend seams

### `sqlite`

Replace `JsonBoardStore` with `SqliteBoardStore`. Tables:

```sql
CREATE TABLE IF NOT EXISTS kanban_columns (
  id    TEXT PRIMARY KEY,
  name  TEXT NOT NULL,
  ordinal INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS kanban_cards (
  id        TEXT PRIMARY KEY,
  column_id TEXT NOT NULL REFERENCES kanban_columns(id),
  title     TEXT NOT NULL,
  body      TEXT NOT NULL,
  ordinal   INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
```

`load` joins both tables and re-assembles `BoardState`. `save` is a
transactional upsert. `reset` truncates both tables and re-seeds the
default Todo / Doing / Done columns.

### `http_json`

Replace with `HttpBoardStore`. `load` → `GET <base>/board`; `save` →
`PUT <base>/board`; `reset` → `DELETE <base>/board`. The base URL +
optional bearer come from the http_json integration's settings.

## FILL markers

| Marker                         | What to fill                                                                    | Example                                                            |
|--------------------------------|---------------------------------------------------------------------------------|--------------------------------------------------------------------|
| `FILL: card-level domain fields` | Extra fields on `Card`: assignee, due date, priority, labels.                  | Add `due_date: Option<u64>` and surface a `<input type="date">`.   |
| `FILL: column customisation`   | Renamed / reordered / added columns beyond Todo / Doing / Done.                  | "Backlog / In Review / Done" for a code-review board.              |
| `FILL: keyboard shortcuts` (opt) | Bind keys for "new card" (n), "move right" (→), etc.                            | Listen for `keydown` on the page; ⌘⏎ to save the active card.     |
| `FILL: per-column WIP limits` (opt) | Cap how many cards each column can hold; warn when exceeded.                  | `Column.wip_limit: Option<u32>` plus a CSS warning class.          |

## Mounting inside the Shell

`app/ui/src/app.rs` must remain:

```rust
#[component]
pub fn App() -> impl IntoView {
    view! { <Shell><KanbanBoardPage/></Shell> }
}
```

Lineage, Feedback Overlay, Canvas overlay, single FAB — all owned by
the shell. Don't re-implement any of them inside the scaffold.

## Verify

```bash
cargo check --workspace
cargo check -p evolvo_ui --target wasm32-unknown-unknown
cargo test  -p evolvo_desktop --lib
```
