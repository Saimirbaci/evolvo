//! SQLite-backed query/exec commands. DB lives at
//! `<workspace>/integrations/sqlite/app.db`. Write guards keep the connection
//! off the async runtime (rusqlite is sync).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rusqlite::{params_from_iter, types::Value as SqlValue, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

pub struct Sqlite {
    conn: Arc<Mutex<Connection>>,
}

impl Sqlite {
    pub fn open(workspace_root: &std::path::Path) -> Result<Self, String> {
        let dir: PathBuf = workspace_root.join("integrations/sqlite");
        std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir: {e}"))?;
        let path = dir.join("app.db");
        let conn = Connection::open(&path).map_err(|e| format!("open db: {e}"))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
               id INTEGER PRIMARY KEY,
               applied_at INTEGER NOT NULL
             );",
        )
        .map_err(|e| format!("init migrations table: {e}"))?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }
}

fn value_to_sql(v: &Json) -> SqlValue {
    match v {
        Json::Null => SqlValue::Null,
        Json::Bool(b) => SqlValue::Integer(*b as i64),
        Json::Number(n) => {
            if let Some(i) = n.as_i64() {
                SqlValue::Integer(i)
            } else if let Some(f) = n.as_f64() {
                SqlValue::Real(f)
            } else {
                SqlValue::Text(n.to_string())
            }
        }
        Json::String(s) => SqlValue::Text(s.clone()),
        other => SqlValue::Text(other.to_string()),
    }
}

fn row_to_json(row: &rusqlite::Row, cols: &[String]) -> Json {
    let mut obj = serde_json::Map::new();
    for (i, name) in cols.iter().enumerate() {
        let v: SqlValue = row.get_unwrap(i);
        let j = match v {
            SqlValue::Null => Json::Null,
            SqlValue::Integer(i) => Json::from(i),
            SqlValue::Real(f) => Json::from(f),
            SqlValue::Text(s) => Json::String(s),
            SqlValue::Blob(b) => Json::String(format!("<blob:{}B>", b.len())),
        };
        obj.insert(name.clone(), j);
    }
    Json::Object(obj)
}

fn reject_multi_statement(sql: &str) -> Result<(), String> {
    let trimmed = sql.trim().trim_end_matches(';');
    if trimmed.contains(';') {
        return Err("multiple statements not allowed".into());
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryArgs {
    pub sql: String,
    #[serde(default)]
    pub params: Vec<Json>,
}

#[tauri::command]
pub async fn sql_query(
    args: QueryArgs,
    state: tauri::State<'_, crate::AppState>,
) -> Result<Vec<Json>, String> {
    reject_multi_statement(&args.sql)?;
    let db = state.sqlite.clone();
    tokio::task::spawn_blocking(move || -> Result<Vec<Json>, String> {
        let conn = db.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare(&args.sql).map_err(|e| format!("prepare: {e}"))?;
        let cols: Vec<String> = stmt
            .column_names()
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        let params: Vec<SqlValue> = args.params.iter().map(value_to_sql).collect();
        let rows = stmt
            .query_map(params_from_iter(params), |row| Ok(row_to_json(row, &cols)))
            .map_err(|e| format!("query: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("collect: {e}"))
    })
    .await
    .map_err(|e| format!("join: {e}"))?
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecResult {
    pub rows_affected: usize,
    pub last_insert_rowid: i64,
}

#[tauri::command]
pub async fn sql_exec(
    args: QueryArgs,
    state: tauri::State<'_, crate::AppState>,
) -> Result<ExecResult, String> {
    reject_multi_statement(&args.sql)?;
    let db = state.sqlite.clone();
    tokio::task::spawn_blocking(move || -> Result<ExecResult, String> {
        let conn = db.conn.lock().map_err(|e| e.to_string())?;
        let params: Vec<SqlValue> = args.params.iter().map(value_to_sql).collect();
        let rows_affected = conn
            .execute(&args.sql, params_from_iter(params))
            .map_err(|e| format!("exec: {e}"))?;
        Ok(ExecResult {
            rows_affected,
            last_insert_rowid: conn.last_insert_rowid(),
        })
    })
    .await
    .map_err(|e| format!("join: {e}"))?
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrateArgs {
    /// `(id, statement)` pairs. `id` is a monotonically-increasing integer —
    /// once applied it's recorded in `schema_migrations` and skipped on reruns.
    pub statements: Vec<(i64, String)>,
}

#[tauri::command]
pub async fn sql_migrate(
    args: MigrateArgs,
    state: tauri::State<'_, crate::AppState>,
) -> Result<usize, String> {
    let db = state.sqlite.clone();
    tokio::task::spawn_blocking(move || -> Result<usize, String> {
        let mut conn = db.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| format!("tx: {e}"))?;
        let mut applied = 0usize;
        for (id, stmt) in &args.statements {
            let already: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM schema_migrations WHERE id = ?1",
                    [id],
                    |r| r.get(0),
                )
                .map_err(|e| format!("check: {e}"))?;
            if already > 0 {
                continue;
            }
            tx.execute_batch(stmt)
                .map_err(|e| format!("migration {id}: {e}"))?;
            tx.execute(
                "INSERT INTO schema_migrations(id, applied_at) VALUES (?1, ?2)",
                rusqlite::params![id, chrono::Utc::now().timestamp_millis()],
            )
            .map_err(|e| format!("record {id}: {e}"))?;
            applied += 1;
        }
        tx.commit().map_err(|e| format!("commit: {e}"))?;
        Ok(applied)
    })
    .await
    .map_err(|e| format!("join: {e}"))?
}
