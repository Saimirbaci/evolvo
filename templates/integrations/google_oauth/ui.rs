//! Leptos `<GoogleSignInButton/>` + interop wrappers.

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
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "event"], js_name = listen)]
    fn listen(event: &str, cb: &Closure<dyn FnMut(JsValue)>) -> js_sys::Promise;
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "opener"], js_name = openUrl, catch)]
    fn open_url(url: &str) -> Result<(), JsValue>;
}

async fn call<T: for<'de> Deserialize<'de>>(cmd: &str, args: impl Serialize) -> Result<T, String> {
    let args = to_value(&args).map_err(|e| e.to_string())?;
    let v = JsFuture::from(invoke(cmd, args))
        .await
        .map_err(|e| e.as_string().unwrap_or_default())?;
    from_value(v).map_err(|e| e.to_string())
}

#[derive(Serialize)]
struct Empty {}
#[derive(Serialize)]
struct CodeArgs<'a> {
    args: CodeInner<'a>,
}
#[derive(Serialize)]
struct CodeInner<'a> {
    code: &'a str,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BeginResult {
    pub auth_url: String,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct UserInfo {
    pub sub: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub picture: Option<String>,
}

pub async fn begin_sign_in() -> Result<BeginResult, String> {
    call("google_begin_sign_in", Empty {}).await
}
pub async fn complete_sign_in(code: &str) -> Result<serde_json::Value, String> {
    call("google_complete_sign_in", CodeArgs { args: CodeInner { code } }).await
}
pub async fn sign_out() -> Result<(), String> {
    call("google_sign_out", Empty {}).await
}
pub async fn is_signed_in() -> Result<bool, String> {
    call("google_is_signed_in", Empty {}).await
}
pub async fn fetch_userinfo() -> Result<UserInfo, String> {
    call("google_fetch_userinfo", Empty {}).await
}

#[component]
pub fn GoogleSignInButton(#[prop(optional)] on_user: Option<Callback<UserInfo>>) -> impl IntoView {
    let signed_in = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);

    Effect::new(move |_| {
        spawn_local(async move {
            if let Ok(b) = is_signed_in().await {
                signed_in.set(b);
            }
            // Subscribe to the loopback redirect event emitted by the plugin.
            let cb = Closure::wrap(Box::new(move |ev: JsValue| {
                // ev is { event, payload: "http://127.0.0.1:XXXX/?code=..." }
                let payload = js_sys::Reflect::get(&ev, &"payload".into()).ok();
                let url = payload.and_then(|p| p.as_string()).unwrap_or_default();
                if let Some(code) = extract_code(&url) {
                    let on_user = on_user;
                    spawn_local(async move {
                        match complete_sign_in(&code).await {
                            Ok(_) => {
                                signed_in.set(true);
                                if let Some(cb) = on_user {
                                    if let Ok(u) = fetch_userinfo().await {
                                        cb.run(u);
                                    }
                                }
                            }
                            Err(e) => error.set(Some(e)),
                        }
                    });
                }
            }) as Box<dyn FnMut(JsValue)>);
            let _ = JsFuture::from(listen("oauth://url", &cb)).await;
            cb.forget();
        });
    });

    let sign_in = move |_| {
        spawn_local(async move {
            match begin_sign_in().await {
                Ok(r) => {
                    if let Err(e) = open_url(&r.auth_url) {
                        error.set(Some(format!("open browser: {e:?}")));
                    }
                }
                Err(e) => error.set(Some(e)),
            }
        });
    };
    let sign_out = move |_| {
        spawn_local(async move {
            let _ = super::ui::sign_out().await;
            signed_in.set(false);
        });
    };

    view! {
        <div>
            {move || if signed_in.get() {
                view! { <button on:click=sign_out>"Sign out of Google"</button> }.into_any()
            } else {
                view! { <button on:click=sign_in>"Sign in with Google"</button> }.into_any()
            }}
            {move || error.get().map(|e| view! { <p style="color:#991b1b;">{e}</p> })}
        </div>
    }
}

fn extract_code(url: &str) -> Option<String> {
    let q = url.split_once('?').map(|(_, q)| q)?;
    for pair in q.split('&') {
        if let Some(v) = pair.strip_prefix("code=") {
            return Some(urldecode(v));
        }
    }
    None
}

fn urldecode(s: &str) -> String {
    // Minimal URL decode — good enough for an OAuth `code` which is url-safe.
    s.replace('+', " ")
}
