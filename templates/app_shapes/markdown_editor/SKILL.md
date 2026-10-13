# markdown_editor

A two-pane markdown editor scaffold: a sidebar lists the user's documents,
a central source pane is a `<textarea>` bound to the active document's
body, a right-hand preview shows the rendered markdown. CRUD is wired
through four Tauri commands and a `DocumentStore` trait so the persistence
layer can be swapped without touching the UI.

## What you get

- One entity: `Document { id, title, body, updatedAt }` (extend in
  `// FILL: domain-specific document fields`).
- Four Tauri commands: `list_documents`, `load_document`, `save_document`,
  `delete_document`.
- One trait: `DocumentStore` (object-safe). Default impl `JsonDocumentStore`
  persists JSON files under `<workspace>/markdown_editor/documents/`.
- Three Leptos components:
  - `MarkdownEditorPage` (in `ui_app.rs`) — the route's root.
  - `DocList` (in `ui_modules/doc_list.rs`) — sidebar.
  - `MarkdownPreview` (in `ui_modules/preview.rs`) — read-only render pane.
- ~80 lines of CSS in `styles.css` (3-column grid, sidebar, prose
  preview).

## Mount points

| What                  | Where                                                              |
|-----------------------|--------------------------------------------------------------------|
| Host commands         | `app/src-tauri/src/app_shapes/markdown_editor.rs`                  |
| invoke_handler!       | `app/src-tauri/src/lib.rs` — register every `#[tauri::command]`    |
| UI page               | `app/ui/src/app_shape_markdown_editor.rs`                          |
| UI components         | `app/ui/src/markdown_editor_doc_list.rs`, `..._preview.rs`         |
| Styles (append)       | `app/ui/styles.css`                                                |

`app/ui/src/app.rs` MUST keep mounting `<Shell>`. Replace `HomePage`'s
content with `<MarkdownEditorPage/>`. The shell still owns the FAB, the
Lineage page, and the Canvas overlay — do not duplicate any of them
inside the scaffold.

## Data interface

```
trait DocumentStore: Send + Sync {
    fn list(&self) -> Result<Vec<Document>, String>;
    fn load(&self, id: &str) -> Result<Document, String>;
    fn save(&self, doc: &Document) -> Result<(), String>;
    fn delete(&self, id: &str) -> Result<bool, String>;
}
```

Default impl: `JsonDocumentStore` (in `host.rs`). Reads/writes
`<workspace>/markdown_editor/documents/{id}.json`. IDs are
`doc-<unix_ms>`. Filenames pass through `crate::store::sanitise_filename`
already (the host's `Store` enforces it).

The four Tauri commands declared in `host.rs` always go through the
`DocumentStore` trait — never the concrete `JsonDocumentStore` — so a
backend seam can replace the impl without touching the commands.

## Backend seams

When the planner records `templates.backendBindings.markdown_editor =
"<integration>"`, the implementer swaps `JsonDocumentStore` for the
integration's adapter. Available seams:

### `sqlite`

Replace the default impl with a `SqliteDocumentStore` that opens the
existing sqlite integration's connection (`AppState.sqlite`) and runs:

```sql
CREATE TABLE IF NOT EXISTS documents (
  id         TEXT PRIMARY KEY,
  title      TEXT NOT NULL,
  body       TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);
```

`list` is `SELECT … ORDER BY updated_at DESC`; `load` is `SELECT WHERE id
= ?1`; `save` is `INSERT … ON CONFLICT(id) DO UPDATE`; `delete` is
`DELETE WHERE id = ?1`. The adapter file
`templates/integrations/sqlite/seam_markdown_editor.rs` is the canonical
location for this code; if absent, write it under
`app/src-tauri/src/app_shapes/markdown_editor_sqlite.rs` and reference it
from the host module's `pub fn build_store(state: &AppState) -> Box<dyn
DocumentStore>` factory.

### `http_json`

Replace the default impl with `HttpDocumentStore { base_url: String,
bearer: Option<String> }`. `list` → `GET <base>/documents`; `load` →
`GET <base>/documents/{id}`; `save` → `PUT <base>/documents/{id}`;
`delete` → `DELETE <base>/documents/{id}`. The `base_url` and (optional)
bearer token come from the user via the http_json integration's settings
panel; the http_json template's `seam_markdown_editor.rs` adapter (when
shipped) wires that state in. Until that adapter exists, hand-roll it
following the http_json template's existing `keyring`-backed token
pattern.

### `google_oauth`

Not yet shipped — would persist documents as `.md` files in an `Evolvo
Notes/` folder in the user's Google Drive (requires the `drive.file`
scope). When implemented, the adapter would live in
`templates/integrations/google_oauth/seam_markdown_editor.rs` and
implement `DocumentStore` by calling Drive's REST API with the OAuth
access token already managed by the google_oauth integration.

If the planner records this binding before the seam exists, BackendImpl
should fall back to the default `JsonDocumentStore` and append a history
entry noting the missing seam — never silently bake in a half-implemented
adapter.

## FILL markers

| Marker                                           | What to fill                                                                                                            | Example                                                          |
|--------------------------------------------------|-------------------------------------------------------------------------------------------------------------------------|------------------------------------------------------------------|
| `FILL: domain-specific document fields`          | Extra fields on the `Document` struct that the user's request implies.                                                  | Add `tags: Vec<String>` and `archived: bool` for a research-notes app. |
| `FILL: domain-specific document actions`         | Buttons / commands beyond CRUD: publish, export, …                                                                      | A `publish_document(id)` command that POSTs to a blog endpoint.  |
| `FILL: list-side affordances`                    | Sidebar refinements: filtering, grouping, sorting controls.                                                             | Add a `<select>` to filter by `tag`.                             |
| `FILL: keyboard shortcuts` (optional)            | Bind hotkeys (e.g. ⌘S to save, ⌘N for new).                                                                              | Listen for `keydown` on the page; on `cmd+s` call `save()`.      |
| `FILL: empty-state copy` (optional)              | The text the sidebar shows when the user has no documents yet.                                                          | "No notes yet — type a title above and hit ⏎."                  |

Any `// FILL:` comment that survives the iteration is a regression — the
validator (planned) greps the diff for the literal string.

## Mounting inside the Shell

`app/ui/src/app.rs` must remain:

```rust
#[component]
pub fn App() -> impl IntoView {
    view! { <Shell><MarkdownEditorPage/></Shell> }
}
```

The single Feedback FAB owned by the shell still opens the Canvas overlay
and the feedback panel together (I-P3b). The Canvas overlay still
annotates *the markdown editor page* — no per-page work needed. Lineage
remains reachable from the app bar (I-P1). The four invariants in
`.claude/rules/common/product-invariants.md` are non-negotiable; deleting
the shell or moving its surfaces inside this scaffold fails the iteration
on sight.

## Verify

```bash
cargo check --workspace
cargo check -p evolvo_ui --target wasm32-unknown-unknown
cargo test  -p evolvo_desktop --lib
```

If any backend binding was applied, also run the integration template's
verify steps (e.g. sqlite's migration smoke test).
