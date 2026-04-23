//! File picker + import helpers (UI side).

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::{Deserialize, Serialize};
use serde_wasm_bindgen::{from_value, to_value};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "core"], js_name = invoke)]
    fn invoke(cmd: &str, args: JsValue) -> js_sys::Promise;
}

async fn call<T: for<'de> Deserialize<'de>>(cmd: &str, args: impl Serialize) -> Result<T, String> {
    let args = to_value(&args).map_err(|e| e.to_string())?;
    let v = JsFuture::from(invoke(cmd, args))
        .await
        .map_err(|e| e.as_string().unwrap_or_default())?;
    from_value(v).map_err(|e| e.to_string())
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FileFilter {
    pub name: String,
    pub extensions: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Table {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

#[derive(Serialize)]
struct PickArgs {
    args: PickInner,
}
#[derive(Serialize)]
struct PickInner {
    filters: Vec<FileFilter>,
}
#[derive(Serialize)]
struct PathArgs {
    args: PathInner,
}
#[derive(Serialize)]
struct PathInner {
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sheet: Option<String>,
}

pub async fn pick_file(filters: Vec<FileFilter>) -> Result<Option<String>, String> {
    call("pick_file", PickArgs { args: PickInner { filters } }).await
}
pub async fn read_csv(path: &str) -> Result<Table, String> {
    call(
        "read_csv",
        PathArgs {
            args: PathInner {
                path: path.into(),
                sheet: None,
            },
        },
    )
    .await
}
pub async fn read_xlsx(path: &str, sheet: Option<&str>) -> Result<Table, String> {
    call(
        "read_xlsx",
        PathArgs {
            args: PathInner {
                path: path.into(),
                sheet: sheet.map(|s| s.to_string()),
            },
        },
    )
    .await
}

#[component]
pub fn ImportDataButton(
    #[prop(into)] on_table: Callback<Table>,
    #[prop(optional, into)] label: Option<String>,
) -> impl IntoView {
    let err = RwSignal::new(None::<String>);
    let label = label.unwrap_or_else(|| "Import data…".into());
    let click = move |_| {
        spawn_local(async move {
            let filters = vec![
                FileFilter {
                    name: "Spreadsheets".into(),
                    extensions: vec!["csv".into(), "xlsx".into(), "xls".into()],
                },
            ];
            match pick_file(filters).await {
                Ok(Some(path)) => {
                    let table = if path.ends_with(".csv") {
                        read_csv(&path).await
                    } else {
                        read_xlsx(&path, None).await
                    };
                    match table {
                        Ok(t) => on_table.run(t),
                        Err(e) => err.set(Some(e)),
                    }
                }
                Ok(None) => {}
                Err(e) => err.set(Some(e)),
            }
        });
    };
    view! {
        <div>
            <button on:click=click>{label}</button>
            {move || err.get().map(|e| view! { <p style="color:#991b1b;">{e}</p> })}
        </div>
    }
}
