//! Loopback HTTP server for receiving third-party webhook callbacks.
//!
//! This is a DEV helper — do not expose beyond `127.0.0.1`, do not rely on
//! it for production delivery guarantees. Events are held in a bounded ring
//! buffer and drained on demand.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::{extract::State, http::HeaderMap, response::IntoResponse, routing::any, Router};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;

const QUEUE_CAP: usize = 500;

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WebhookEvent {
    pub path: String,
    pub method: String,
    pub received_at_ms: u64,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

#[derive(Default)]
pub struct WebhookIngest {
    inner: Mutex<Option<Running>>,
    queue: Arc<Mutex<VecDeque<WebhookEvent>>>,
}

struct Running {
    addr: SocketAddr,
    shutdown: tokio::sync::oneshot::Sender<()>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartArgs {
    pub path_prefix: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartResult {
    pub url: String,
}

#[tauri::command]
pub async fn webhook_start(
    args: StartArgs,
    state: tauri::State<'_, crate::AppState>,
) -> Result<StartResult, String> {
    let ingest = state.webhook.clone();
    {
        let guard = ingest.inner.lock().map_err(|e| e.to_string())?;
        if let Some(r) = guard.as_ref() {
            return Ok(StartResult {
                url: format!("http://{}/{}", r.addr, args.path_prefix.trim_matches('/')),
            });
        }
    }
    let queue = ingest.queue.clone();
    let app = Router::new()
        .route("/*rest", any(handle))
        .with_state(queue.clone());
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("bind: {e}"))?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = rx.await;
            })
            .await;
    });
    {
        let mut guard = ingest.inner.lock().map_err(|e| e.to_string())?;
        *guard = Some(Running { addr, shutdown: tx });
    }
    Ok(StartResult {
        url: format!("http://{}/{}", addr, args.path_prefix.trim_matches('/')),
    })
}

#[tauri::command]
pub async fn webhook_stop(state: tauri::State<'_, crate::AppState>) -> Result<(), String> {
    let ingest = state.webhook.clone();
    let mut guard = ingest.inner.lock().map_err(|e| e.to_string())?;
    if let Some(r) = guard.take() {
        let _ = r.shutdown.send(());
    }
    Ok(())
}

#[tauri::command]
pub async fn webhook_drain(
    state: tauri::State<'_, crate::AppState>,
) -> Result<Vec<WebhookEvent>, String> {
    let ingest = state.webhook.clone();
    let mut q = ingest.queue.lock().map_err(|e| e.to_string())?;
    Ok(q.drain(..).collect())
}

async fn handle(
    State(queue): State<Arc<Mutex<VecDeque<WebhookEvent>>>>,
    headers: HeaderMap,
    method: axum::http::Method,
    uri: axum::http::Uri,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let ev = WebhookEvent {
        path: uri.path().to_string(),
        method: method.to_string(),
        received_at_ms: now_ms(),
        headers: headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or_default().to_string()))
            .collect(),
        body: String::from_utf8_lossy(&body).into_owned(),
    };
    if let Ok(mut q) = queue.lock() {
        if q.len() >= QUEUE_CAP {
            q.pop_front();
        }
        q.push_back(ev);
    }
    axum::http::StatusCode::OK
}
