# file_picker — open/save dialogs + CSV/XLSX import

Covers the whole "import my data" genre of desktop apps. Uses `tauri-plugin-dialog` for native file pickers and `calamine` + `csv` for spreadsheet parsing, all on the host side so the WASM bundle stays small.

## What you get

- `host.rs` — `pick_file(filters)`, `save_file(filters, contents)`, `read_csv(path)`, `read_xlsx(path, sheet)`. Each returns a `{ headers, rows }` shape for tabular imports.
- `ui.rs` — `<ImportDataButton on_rows=...>` Leptos component that wires the dialog, parse, and returns rows to the caller.

## Wire-up

1. Deps: `tauri-plugin-dialog`, `calamine`, `csv`.
2. Register the plugin in `lib.rs`: `.plugin(tauri_plugin_dialog::init())`.
3. Copy `host.rs` + `ui.rs`, register commands.

## Scope

- Tabular formats only. For PDFs / images / arbitrary binary, use `pick_file` to get the path and call a specialised parser.
- Row cap: 1M rows. Anything larger should stream — swap the template for `calamine`'s iterator API.

## Security

- File paths returned by the native dialog are trusted (user chose them).
- Don't ever take a `path: String` from the UI and pass it to `read_csv` without a dialog round-trip — that's a sandbox escape.
