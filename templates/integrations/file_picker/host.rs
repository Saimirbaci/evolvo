//! Native file picker + CSV/XLSX import helpers.

use std::path::PathBuf;

use calamine::{open_workbook_auto, Data, Reader};
use serde::{Deserialize, Serialize};
use tauri_plugin_dialog::DialogExt;

const ROW_CAP: usize = 1_000_000;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PickArgs {
    #[serde(default)]
    pub filters: Vec<FileFilter>,
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FileFilter {
    pub name: String,
    pub extensions: Vec<String>,
}

#[tauri::command]
pub async fn pick_file(
    args: PickArgs,
    app: tauri::AppHandle,
) -> Result<Option<String>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut dlg = app.dialog().file();
    for f in args.filters {
        let exts: Vec<&str> = f.extensions.iter().map(|s| s.as_str()).collect();
        dlg = dlg.add_filter(&f.name, &exts);
    }
    dlg.pick_file(move |p| {
        let _ = tx.send(p.and_then(|p| p.into_path().ok()));
    });
    let path = rx.await.map_err(|e| e.to_string())?;
    Ok(path.map(|p: PathBuf| p.display().to_string()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveArgs {
    #[serde(default)]
    pub filters: Vec<FileFilter>,
    pub contents: String,
    #[serde(default)]
    pub suggested_name: Option<String>,
}

#[tauri::command]
pub async fn save_file(
    args: SaveArgs,
    app: tauri::AppHandle,
) -> Result<Option<String>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut dlg = app.dialog().file();
    for f in args.filters {
        let exts: Vec<&str> = f.extensions.iter().map(|s| s.as_str()).collect();
        dlg = dlg.add_filter(&f.name, &exts);
    }
    if let Some(n) = args.suggested_name.as_deref() {
        dlg = dlg.set_file_name(n);
    }
    dlg.save_file(move |p| {
        let _ = tx.send(p.and_then(|p| p.into_path().ok()));
    });
    let path = rx.await.map_err(|e| e.to_string())?;
    if let Some(p) = &path {
        std::fs::write(p, args.contents.as_bytes())
            .map_err(|e| format!("write: {e}"))?;
    }
    Ok(path.map(|p| p.display().to_string()))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Table {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadCsvArgs {
    pub path: String,
}

#[tauri::command]
pub async fn read_csv(args: ReadCsvArgs) -> Result<Table, String> {
    tokio::task::spawn_blocking(move || -> Result<Table, String> {
        let mut rdr = csv::Reader::from_path(&args.path).map_err(|e| format!("csv open: {e}"))?;
        let headers = rdr
            .headers()
            .map_err(|e| format!("csv headers: {e}"))?
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        let mut rows = Vec::new();
        for (i, rec) in rdr.records().enumerate() {
            if i >= ROW_CAP {
                return Err(format!("row cap {ROW_CAP} exceeded"));
            }
            let rec = rec.map_err(|e| format!("csv row {i}: {e}"))?;
            rows.push(rec.iter().map(|s| s.to_string()).collect());
        }
        Ok(Table { headers, rows })
    })
    .await
    .map_err(|e| format!("join: {e}"))?
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadXlsxArgs {
    pub path: String,
    #[serde(default)]
    pub sheet: Option<String>,
}

#[tauri::command]
pub async fn read_xlsx(args: ReadXlsxArgs) -> Result<Table, String> {
    tokio::task::spawn_blocking(move || -> Result<Table, String> {
        let mut wb = open_workbook_auto(&args.path).map_err(|e| format!("xlsx open: {e}"))?;
        let sheet_name = args
            .sheet
            .clone()
            .or_else(|| wb.sheet_names().into_iter().next())
            .ok_or_else(|| "no sheets".to_string())?;
        let range = wb
            .worksheet_range(&sheet_name)
            .map_err(|e| format!("sheet: {e}"))?;
        let mut rows_iter = range.rows();
        let headers: Vec<String> = rows_iter
            .next()
            .map(|r| r.iter().map(cell_to_string).collect())
            .unwrap_or_default();
        let mut rows = Vec::new();
        for (i, row) in rows_iter.enumerate() {
            if i >= ROW_CAP {
                return Err(format!("row cap {ROW_CAP} exceeded"));
            }
            rows.push(row.iter().map(cell_to_string).collect());
        }
        Ok(Table { headers, rows })
    })
    .await
    .map_err(|e| format!("join: {e}"))?
}

fn cell_to_string(c: &Data) -> String {
    match c {
        Data::Empty => String::new(),
        Data::String(s) => s.clone(),
        Data::Float(f) => f.to_string(),
        Data::Int(i) => i.to_string(),
        Data::Bool(b) => b.to_string(),
        Data::DateTime(dt) => dt.to_string(),
        Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
        Data::Error(e) => format!("#ERR:{e:?}"),
    }
}
