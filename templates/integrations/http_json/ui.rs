//! Typed interop wrappers for the `http_json` integration.

use std::collections::HashMap;

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

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct HttpResult {
    pub status: u16,
    pub body: serde_json::Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RequestArgs<'a> {
    args: RequestInner<'a>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RequestInner<'a> {
    method: &'a str,
    url: &'a str,
    headers: HashMap<String, String>,
    bearer_ref: Option<&'a str>,
    body: Option<serde_json::Value>,
}

pub async fn fetch_json(
    method: &str,
    url: &str,
    headers: HashMap<String, String>,
    bearer_ref: Option<&str>,
    body: Option<serde_json::Value>,
) -> Result<HttpResult, String> {
    call(
        "http_request",
        RequestArgs {
            args: RequestInner {
                method,
                url,
                headers,
                bearer_ref,
                body,
            },
        },
    )
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BearerArgs<'a> {
    args: BearerInner<'a>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BearerInner<'a> {
    bearer_ref: &'a str,
    token: Option<&'a str>,
}

pub async fn set_bearer(bearer_ref: &str, token: &str) -> Result<(), String> {
    call(
        "http_set_bearer",
        BearerArgs {
            args: BearerInner {
                bearer_ref,
                token: Some(token),
            },
        },
    )
    .await
}
pub async fn has_bearer(bearer_ref: &str) -> Result<bool, String> {
    call(
        "http_has_bearer",
        BearerArgs {
            args: BearerInner {
                bearer_ref,
                token: None,
            },
        },
    )
    .await
}
pub async fn clear_bearer(bearer_ref: &str) -> Result<(), String> {
    call(
        "http_clear_bearer",
        BearerArgs {
            args: BearerInner {
                bearer_ref,
                token: None,
            },
        },
    )
    .await
}
