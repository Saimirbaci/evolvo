# sqlite — local relational DB

`rusqlite` + a tiny migration runner. DB file at `<workspace>/integrations/sqlite/app.db`. Good default when the NewApp needs joins / indexes / transactions and JSON-per-entity doesn't cut it.

## What you get

- `host.rs` — `sql_query(sql, params) -> rows` and `sql_exec(sql, params) -> rows_affected`, plus `sql_migrate(statements)` that records applied migrations in a `schema_migrations` table.
- Synchronous at the SQLite boundary, wrapped in `tokio::task::spawn_blocking` so the Tauri runtime stays responsive.

## Wire-up

1. Deps: `rusqlite = { version = "0.32", features = ["bundled"] }`.
2. Copy `host.rs` → `app/src-tauri/src/integrations/sqlite.rs`, register in `invoke_handler`.
3. On startup, call `sql_migrate` with your schema (see template).

## Safety

- Parameters go through `rusqlite`'s parameter binding — **never** concatenate user input into SQL strings. The commands reject queries with trailing semicolons / multiple statements.
- The DB path is inside the workspace root; not reachable from WASM except through these commands.

## When NOT to use this

- For simple key/value or document data, stick with the host `Store` (JSON files). SQLite is worth the dependency only when relational queries materially simplify the code.
