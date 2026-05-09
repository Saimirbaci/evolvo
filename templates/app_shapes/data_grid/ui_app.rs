//! data_grid — UI scaffold (the NewApp content area).
//!
//! Drop into `app/ui/src/app_shape_data_grid.rs`, declare the new module
//! in `app/ui/src/main.rs`, and rewrite `app/ui/src/app.rs` so its `App`
//! component wraps `<DataGridPage/>` inside `<Shell>`.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{json, Value as Json};
use wasm_bindgen::JsCast;

use crate::interop;
use crate::types::Row;

/// One column in the grid. Edit `default_columns()` below at the
/// `// FILL: column schema` marker to declare the iteration's columns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnDef {
    pub id: &'static str,
    pub label: &'static str,
}

fn default_columns() -> Vec<ColumnDef> {
    // FILL: column schema — replace with the columns the user's request
    // implies. Keep `id` snake_case (the JSON key in `cells`) and `label`
    // human-readable.
    vec![
        ColumnDef {
            id: "name",
            label: "Name",
        },
        ColumnDef {
            id: "email",
            label: "Email",
        },
        ColumnDef {
            id: "notes",
            label: "Notes",
        },
    ]
}

#[component]
pub fn DataGridPage() -> impl IntoView {
    let columns: Vec<ColumnDef> = default_columns();
    let rows: RwSignal<Vec<Row>> = RwSignal::new(Vec::new());
    let load_error: RwSignal<Option<String>> = RwSignal::new(None);
    let filter: RwSignal<String> = RwSignal::new(String::new());
    let sort_column: RwSignal<Option<String>> = RwSignal::new(None);
    let sort_asc: RwSignal<bool> = RwSignal::new(true);

    let reload: RwSignal<u32> = RwSignal::new(0);
    Effect::new(move |_| {
        let _ = reload.get();
        spawn_local(async move {
            match interop::list_rows().await {
                Ok(rs) => rows.set(rs),
                Err(e) => load_error.set(Some(e)),
            }
        });
    });

    let add_row = move || {
        let cols = default_columns();
        let mut cells = serde_json::Map::new();
        for c in &cols {
            cells.insert(c.id.to_string(), Json::String(String::new()));
        }
        let payload_cells = Json::Object(cells);
        spawn_local(async move {
            match interop::save_row(None, payload_cells).await {
                Ok(_) => reload.update(|v| *v = v.wrapping_add(1)),
                Err(e) => load_error.set(Some(e)),
            }
        });
    };

    let delete_row = move |id: String| {
        spawn_local(async move {
            match interop::delete_row(&id).await {
                Ok(_) => reload.update(|v| *v = v.wrapping_add(1)),
                Err(e) => load_error.set(Some(e)),
            }
        });
    };

    let update_cell = move |row_id: String, column_id: String, value: String| {
        let current = rows.get_untracked();
        let Some(row) = current.iter().find(|r| r.id == row_id) else {
            return;
        };
        let mut cells = match row.cells.clone() {
            Json::Object(m) => m,
            _ => serde_json::Map::new(),
        };
        cells.insert(column_id, Json::String(value));
        let updated_cells = Json::Object(cells);
        spawn_local(async move {
            match interop::save_row(Some(&row_id), updated_cells).await {
                Ok(_) => reload.update(|v| *v = v.wrapping_add(1)),
                Err(e) => load_error.set(Some(e)),
            }
        });
    };

    let toggle_sort = move |column_id: String| {
        let same_column = sort_column.get_untracked().as_deref() == Some(column_id.as_str());
        if same_column {
            sort_asc.update(|v| *v = !*v);
        } else {
            sort_column.set(Some(column_id));
            sort_asc.set(true);
        }
    };

    // FILL: keyboard shortcuts — Tab/Shift-Tab between cells, ↩ to commit
    // edits, Esc to cancel, ⌘N for add-row.

    let columns_for_header = columns.clone();
    let columns_for_body = columns.clone();

    view! {
        <div class="grid-page">
            <header class="grid-toolbar">
                <input
                    class="grid-filter"
                    placeholder="Filter rows…"
                    type="text"
                    prop:value=move || filter.get()
                    on:input=move |ev| {
                        let t = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                        if let Some(t) = t { filter.set(t.value()); }
                    }
                    aria-label="Filter rows"
                />
                <button
                    class="primary-btn"
                    on:click=move |_| add_row.clone()()
                    aria-label="Add row"
                    title="Append a blank row"
                >
                    "+ Add row"
                </button>
                // FILL: bulk actions — multi-row select + delete / duplicate
                // / export buttons.
            </header>

            {move || match load_error.get() {
                Some(e) => view! {
                    <div class="grid-error" role="alert">{e}</div>
                }.into_any(),
                None => view! { <span></span> }.into_any(),
            }}

            <div class="grid-scroll">
                <table class="grid-table">
                    <thead>
                        <tr>
                            {columns_for_header.iter().map(|c| {
                                let id = c.id.to_string();
                                let id_for_click = id.clone();
                                let id_for_indicator = id.clone();
                                let label = c.label;
                                view! {
                                    <th
                                        class="grid-th"
                                        on:click=move |_| toggle_sort.clone()(id_for_click.clone())
                                    >
                                        <span>{label}</span>
                                        {move || {
                                            if sort_column.get().as_deref() == Some(id_for_indicator.as_str()) {
                                                let arrow = if sort_asc.get() { "↑" } else { "↓" };
                                                view! { <span class="grid-sort">{arrow}</span> }.into_any()
                                            } else {
                                                view! { <span></span> }.into_any()
                                            }
                                        }}
                                    </th>
                                }
                            }).collect_view()}
                            <th class="grid-th grid-th-actions" aria-label="Row actions"></th>
                        </tr>
                    </thead>
                    <tbody>
                        {move || {
                            let mut filtered = rows.get();
                            let needle = filter.get().trim().to_lowercase();
                            if !needle.is_empty() {
                                filtered.retain(|r| {
                                    let s = r.cells.to_string().to_lowercase();
                                    s.contains(&needle)
                                });
                            }
                            if let Some(col) = sort_column.get() {
                                let asc = sort_asc.get();
                                filtered.sort_by(|a, b| {
                                    let av = cell_string(&a.cells, &col);
                                    let bv = cell_string(&b.cells, &col);
                                    if asc { av.cmp(&bv) } else { bv.cmp(&av) }
                                });
                            }
                            if filtered.is_empty() {
                                let span = (columns_for_body.len() + 1).to_string();
                                return view! {
                                    <tr><td class="grid-empty" colspan=span>"No rows."</td></tr>
                                }.into_any();
                            }
                            let cols = columns_for_body.clone();
                            filtered.into_iter().map(|row| {
                                let row_id = row.id.clone();
                                let row_id_for_delete = row_id.clone();
                                let cells = row.cells.clone();
                                view! {
                                    <tr class="grid-row">
                                        {cols.iter().map(|c| {
                                            let cid = c.id.to_string();
                                            let cid_for_input = cid.clone();
                                            let row_id_for_input = row_id.clone();
                                            let value = cell_string(&cells, &cid);
                                            view! {
                                                <td class="grid-td">
                                                    // FILL: cell editor for non-string columns
                                                    // — swap the <input> for a <select> /
                                                    // <input type="date"> / <input type="number">
                                                    // depending on the column's type.
                                                    <input
                                                        class="grid-cell-input"
                                                        type="text"
                                                        prop:value=value
                                                        on:change=move |ev| {
                                                            let t = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                                            if let Some(t) = t {
                                                                update_cell.clone()(row_id_for_input.clone(), cid_for_input.clone(), t.value());
                                                            }
                                                        }
                                                        aria-label=cid.clone()
                                                    />
                                                </td>
                                            }
                                        }).collect_view()}
                                        <td class="grid-td grid-td-actions">
                                            <button
                                                class="grid-row-delete"
                                                on:click=move |_| delete_row.clone()(row_id_for_delete.clone())
                                                aria-label="Delete row"
                                                title="Delete this row"
                                            >
                                                "×"
                                            </button>
                                        </td>
                                    </tr>
                                }
                            }).collect_view().into_any()
                        }}
                    </tbody>
                </table>
            </div>
        </div>
    }
}

fn cell_string(cells: &Json, column_id: &str) -> String {
    cells
        .get(column_id)
        .and_then(|v| v.as_str().map(|s| s.to_string()).or_else(|| Some(v.to_string())))
        .unwrap_or_default()
}

// Keep `json!` import used so `cargo check` doesn't flag it.
#[allow(dead_code)]
fn _example_cell() -> Json {
    json!({})
}
