# survey_form

Two-mode UI: **Fill** (the user fills the form and submits a `Response`)
and **Review** (the operator browses past responses). The form's field
schema is declared in code at the `// FILL: form schema` marker, so the
agent can shape it to the user's request without changing the wire types
(`values: serde_json::Value`).

## What you get

- One entity: `Response { id, values, submittedAt }`.
- Four Tauri commands: `list_responses`, `load_response`, `save_response`,
  `delete_response`.
- One trait: `ResponseStore` (object-safe). Default impl
  `JsonResponseStore` persists `<workspace>/survey_form/responses/{id}.json`.
- One Leptos component: `SurveyFormPage` (in `ui_app.rs`). Renders the
  form from a `Vec<FieldDef>` constant + a Review tab listing past
  responses.
- ~80 lines of CSS.

## Mount points

| What             | Where                                                   |
|------------------|---------------------------------------------------------|
| Host commands    | `app/src-tauri/src/app_shapes/survey_form.rs`           |
| invoke_handler!  | `app/src-tauri/src/lib.rs`                              |
| UI page          | `app/ui/src/app_shape_survey_form.rs`                   |
| Styles (append)  | `app/ui/styles.css`                                     |

## Data interface

```
trait ResponseStore: Send + Sync {
    fn list(&self) -> Result<Vec<Response>, String>;
    fn load(&self, id: &str) -> Result<Response, String>;
    fn save(&self, response: &Response) -> Result<(), String>;
    fn delete(&self, id: &str) -> Result<bool, String>;
}
```

## Backend seams

### `sqlite`

Replace with `SqliteResponseStore`:

```sql
CREATE TABLE IF NOT EXISTS responses (
  id           TEXT PRIMARY KEY,
  values_json  TEXT NOT NULL,
  submitted_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS responses_by_date ON responses(submitted_at DESC);
```

Worth doing once the operator wants to slice responses by date or by
field value (`json_extract(values_json, '$.<field>')`).

### `http_json`

When bound, `save_response` ALSO POSTs to a configured remote endpoint
after the local persist. Useful when the form is the local end of a
remote data pipeline; the remote URL + bearer token come from the
http_json integration's settings.

### `webhook_ingest`

Inverts the relationship: a remote system POSTs to the loopback server,
the seam unpacks the body and calls `save_response` locally. Useful when
the local app aggregates submissions sent from elsewhere (a website's
"contact us" form, an IoT device, …).

## FILL markers

| Marker                                | What to fill                                                                                                       |
|---------------------------------------|--------------------------------------------------------------------------------------------------------------------|
| `FILL: form schema`                   | The `Vec<FieldDef>` declared at the top of `ui_app.rs` — every field shown to the user.                            |
| `FILL: validation rules` (opt)        | Per-field validation (required, regex, length) checked before save.                                                |
| `FILL: thank-you copy` (opt)          | The string shown after a successful submission.                                                                    |
| `FILL: keyboard shortcuts` (opt)      | ⌘⏎ to submit.                                                                                                     |

## Mounting inside the Shell

```rust
#[component]
pub fn App() -> impl IntoView {
    view! { <Shell><SurveyFormPage/></Shell> }
}
```

## Verify

```bash
cargo check --workspace
cargo check -p evolvo_ui --target wasm32-unknown-unknown
cargo test  -p evolvo_desktop --lib
```
