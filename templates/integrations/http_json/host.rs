//! Generic host-side JSON HTTP helper. Bearer tokens are looked up by
//! *reference* so they never transit the WASM boundary in cleartext.

use std::collections::HashMap;

use keyring::Entry;
use serde::{Deserialize, Serialize};

const KEYRING_SERVICE: &str = "evolvo.http";

fn entry(bearer_ref: &str) -> Result<Entry, String> {
    Entry::new(KEYRING_SERVICE, bearer_ref).map_err(|e| format!("keyring: {e}"))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestArgs {
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub bearer_ref: Option<String>,
    #[serde(default)]
    pub body: Option<serde_json::Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestResult {
    pub status: u16,
    pub body: serde_json::Value,
}

#[tauri::command]
pub async fn http_request(args: RequestArgs) -> Result<RequestResult, String> {
    let method = args
        .method
        .parse::<reqwest::Method>()
        .map_err(|e| format!("bad method: {e}"))?;
    let mut req = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| format!("client: {e}"))?
        .request(method, &args.url);
    for (k, v) in args.headers {
        req = req.header(k, v);
    }
    if let Some(r) = args.bearer_ref.as_deref() {
        if !r.is_empty() {
            let token = entry(r)?
                .get_password()
                .map_err(|e| format!("no bearer {r}: {e}"))?;
            req = req.bearer_auth(token);
        }
    }
    if let Some(body) = args.body {
        req = req.json(&body);
    }
    let res = req.send().await.map_err(|e| format!("send: {e}"))?;
    let status = res.status().as_u16();
    let body: serde_json::Value = res
        .json()
        .await
        .unwrap_or_else(|_| serde_json::Value::Null);
    Ok(RequestResult { status, body })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BearerArgs {
    pub bearer_ref: String,
    #[serde(default)]
    pub token: Option<String>,
}

#[tauri::command]
pub async fn http_set_bearer(args: BearerArgs) -> Result<(), String> {
    let token = args
        .token
        .ok_or_else(|| "token is required".to_string())?;
    entry(&args.bearer_ref)?
        .set_password(&token)
        .map_err(|e| format!("keyring set: {e}"))
}

#[tauri::command]
pub async fn http_has_bearer(args: BearerArgs) -> Result<bool, String> {
    Ok(entry(&args.bearer_ref)?.get_password().is_ok())
}

#[tauri::command]
pub async fn http_clear_bearer(args: BearerArgs) -> Result<(), String> {
    let _ = entry(&args.bearer_ref)?.delete_credential();
    Ok(())
}
