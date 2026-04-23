//! Leptos UI for the OpenRouter chat panel.
//!
//! Mount with `<ChatPanel app_name="YourApp".into()/>` from any NewApp page.
//! The first time it opens it checks `openrouter_has_api_key` — if false it
//! shows a modal asking the user to paste an OpenRouter key.
//!
//! Styling: keeps to three utility classes (`.chat`, `.chat-msg`, `.chat-msg-user`)
//! so the NewApp can theme it; everything else is inline style for drop-in use.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::{Deserialize, Serialize};
use serde_wasm_bindgen::{from_value, to_value};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

// ───────── Wire types (mirror host) ────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub role: Role,
    pub content: String,
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSession {
    pub id: String,
    pub app_name: String,
    pub model: String,
    pub title: Option<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    #[serde(default)]
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: String,
    pub app_name: String,
    pub model: String,
    pub title: Option<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub message_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub context_length: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendMessageResult {
    pub session: ChatSession,
    pub assistant_message: Message,
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

// ───────── Interop wrappers ────────────────────────────────────────────────

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "core"], js_name = invoke)]
    fn invoke(cmd: &str, args: JsValue) -> js_sys::Promise;
}

async fn call<T: for<'de> Deserialize<'de>>(
    cmd: &str,
    args: impl Serialize,
) -> Result<T, String> {
    let args = to_value(&args).map_err(|e| e.to_string())?;
    let fut = JsFuture::from(invoke(cmd, args));
    match fut.await {
        Ok(v) => from_value(v).map_err(|e| e.to_string()),
        Err(e) => Err(e.as_string().unwrap_or_else(|| format!("{e:?}"))),
    }
}

#[derive(Serialize)]
struct NoArgs {}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionIdArgs<'a> {
    session_id: &'a str,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateArgs<'a> {
    args: CreateInner<'a>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateInner<'a> {
    app_name: &'a str,
    model: Option<&'a str>,
}
#[derive(Serialize)]
struct SendArgs<'a> {
    args: SendInner<'a>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SendInner<'a> {
    session_id: &'a str,
    user_message: &'a str,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LoadArgs<'a> {
    args: SessionIdInner<'a>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionIdInner<'a> {
    session_id: &'a str,
}
#[derive(Serialize)]
struct KeyArgs<'a> {
    key: &'a str,
}

pub async fn has_api_key() -> Result<bool, String> {
    call("openrouter_has_api_key", NoArgs {}).await
}
pub async fn set_api_key(key: &str) -> Result<(), String> {
    call("openrouter_set_api_key", KeyArgs { key }).await
}
pub async fn clear_api_key() -> Result<(), String> {
    call("openrouter_clear_api_key", NoArgs {}).await
}
pub async fn create_session(app_name: &str, model: Option<&str>) -> Result<ChatSession, String> {
    call(
        "openrouter_create_session",
        CreateArgs {
            args: CreateInner { app_name, model },
        },
    )
    .await
}
pub async fn list_sessions() -> Result<Vec<SessionSummary>, String> {
    call("openrouter_list_sessions", NoArgs {}).await
}
pub async fn load_session(session_id: &str) -> Result<ChatSession, String> {
    call(
        "openrouter_load_session",
        LoadArgs {
            args: SessionIdInner { session_id },
        },
    )
    .await
}
pub async fn delete_session(session_id: &str) -> Result<(), String> {
    call(
        "openrouter_delete_session",
        LoadArgs {
            args: SessionIdInner { session_id },
        },
    )
    .await
}
pub async fn send_message(
    session_id: &str,
    user_message: &str,
) -> Result<SendMessageResult, String> {
    call(
        "openrouter_send_message",
        SendArgs {
            args: SendInner {
                session_id,
                user_message,
            },
        },
    )
    .await
}
pub async fn list_models() -> Result<Vec<ModelInfo>, String> {
    call("openrouter_list_models", NoArgs {}).await
}

// ───────── Component ───────────────────────────────────────────────────────

#[component]
pub fn ChatPanel(
    #[prop(into)] app_name: String,
    #[prop(optional, into)] default_model: Option<String>,
) -> impl IntoView {
    let app_name = StoredValue::new(app_name);
    let default_model = StoredValue::new(default_model);

    let has_key = RwSignal::new(None::<bool>);
    let sessions = RwSignal::new(Vec::<SessionSummary>::new());
    let current = RwSignal::new(None::<ChatSession>);
    let draft = RwSignal::new(String::new());
    let sending = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);

    let refresh_sessions = move || {
        spawn_local(async move {
            match list_sessions().await {
                Ok(xs) => sessions.set(xs),
                Err(e) => error.set(Some(e)),
            }
        });
    };

    // Initial load: key status + session list.
    Effect::new(move |_| {
        spawn_local(async move {
            match has_api_key().await {
                Ok(b) => has_key.set(Some(b)),
                Err(e) => {
                    has_key.set(Some(false));
                    error.set(Some(e));
                }
            }
        });
        refresh_sessions();
    });

    let new_session = move |_| {
        let name = app_name.get_value();
        let model = default_model.get_value();
        spawn_local(async move {
            match create_session(&name, model.as_deref()).await {
                Ok(s) => {
                    current.set(Some(s));
                    refresh_sessions();
                }
                Err(e) => error.set(Some(e)),
            }
        });
    };

    let open_session = move |id: String| {
        spawn_local(async move {
            match load_session(&id).await {
                Ok(s) => current.set(Some(s)),
                Err(e) => error.set(Some(e)),
            }
        });
    };

    let send = move |_| {
        let text = draft.get().trim().to_string();
        if text.is_empty() || sending.get() {
            return;
        }
        let Some(sess) = current.get() else {
            error.set(Some("No active session — create one first".into()));
            return;
        };
        sending.set(true);
        draft.set(String::new());
        let sid = sess.id.clone();
        spawn_local(async move {
            match send_message(&sid, &text).await {
                Ok(res) => {
                    current.set(Some(res.session));
                    refresh_sessions();
                }
                Err(e) => error.set(Some(e)),
            }
            sending.set(false);
        });
    };

    let save_key = move |val: String| {
        spawn_local(async move {
            if let Err(e) = set_api_key(&val).await {
                error.set(Some(e));
                return;
            }
            has_key.set(Some(true));
            error.set(None);
        });
    };

    view! {
        <div class="chat" style="display:flex; height:100%; min-height:480px; font-family:inherit;">
            {move || match has_key.get() {
                None => view! { <div style="padding:1rem;">Loading…</div> }.into_any(),
                Some(false) => view! { <ApiKeyPrompt on_save=save_key/> }.into_any(),
                Some(true) => view! {
                    <aside style="width:240px; border-right:1px solid #e4e4e7; display:flex; flex-direction:column;">
                        <button on:click=new_session style="margin:.5rem; padding:.5rem;">"+ New chat"</button>
                        <ul style="list-style:none; padding:0; margin:0; overflow-y:auto;">
                            <For
                                each=move || sessions.get()
                                key=|s| s.id.clone()
                                children=move |s: SessionSummary| {
                                    let id = s.id.clone();
                                    let label = s.title.clone().unwrap_or_else(|| format!("session {}", &s.id[..8]));
                                    view! {
                                        <li style="padding:.25rem .5rem;">
                                            <button
                                                on:click=move |_| open_session(id.clone())
                                                style="all:unset; cursor:pointer; display:block; width:100%; padding:.25rem;"
                                            >
                                                {label}
                                                <small style="display:block; color:#71717a;">{s.model}</small>
                                            </button>
                                        </li>
                                    }
                                }
                            />
                        </ul>
                    </aside>
                    <section style="flex:1; display:flex; flex-direction:column;">
                        <header style="padding:.5rem 1rem; border-bottom:1px solid #e4e4e7; display:flex; justify-content:space-between; align-items:center;">
                            <span>{move || current.get().map(|s| s.model).unwrap_or_else(|| "no session".into())}</span>
                            <small style="color:#71717a;">{app_name.get_value()}</small>
                        </header>
                        <div style="flex:1; overflow-y:auto; padding:1rem; display:flex; flex-direction:column; gap:.75rem;">
                            {move || current.get().map(|s| {
                                s.messages.into_iter().map(|m| {
                                    let is_user = m.role == Role::User;
                                    let bg = if is_user { "#2563eb" } else { "#f4f4f5" };
                                    let fg = if is_user { "#fff" } else { "#18181b" };
                                    let align = if is_user { "flex-end" } else { "flex-start" };
                                    view! {
                                        <div
                                            class=if is_user { "chat-msg chat-msg-user" } else { "chat-msg" }
                                            style=format!("align-self:{align}; background:{bg}; color:{fg}; padding:.5rem .75rem; border-radius:.5rem; max-width:80%; white-space:pre-wrap;")
                                        >
                                            {m.content}
                                        </div>
                                    }
                                }).collect_view()
                            })}
                            {move || sending.get().then(|| view! {
                                <div style="align-self:flex-start; color:#71717a; font-style:italic;">"…"</div>
                            })}
                        </div>
                        {move || error.get().map(|e| view! {
                            <div style="padding:.5rem 1rem; background:#fee2e2; color:#991b1b; border-top:1px solid #fecaca;">
                                {e}
                                <button on:click=move |_| error.set(None) style="float:right; all:unset; cursor:pointer;">"×"</button>
                            </div>
                        })}
                        <footer style="padding:.5rem; border-top:1px solid #e4e4e7; display:flex; gap:.5rem;">
                            <textarea
                                prop:value=move || draft.get()
                                on:input=move |ev| draft.set(event_target_value(&ev))
                                placeholder="Message the assistant…"
                                rows="2"
                                style="flex:1; padding:.5rem; resize:vertical;"
                                on:keydown=move |ev| {
                                    if ev.key() == "Enter" && !ev.shift_key() {
                                        ev.prevent_default();
                                        send(ev.into());
                                    }
                                }
                            ></textarea>
                            <button on:click=send disabled=move || sending.get() || current.get().is_none()>
                                "Send"
                            </button>
                        </footer>
                    </section>
                }.into_any(),
            }}
        </div>
    }
}

#[component]
fn ApiKeyPrompt(on_save: impl Fn(String) + 'static + Copy) -> impl IntoView {
    let val = RwSignal::new(String::new());
    view! {
        <div style="margin:auto; padding:2rem; max-width:420px; display:flex; flex-direction:column; gap:.75rem;">
            <h3 style="margin:0;">"Connect OpenRouter"</h3>
            <p style="margin:0; color:#52525b; font-size:.875rem;">
                "Paste your OpenRouter API key to enable the in-app assistant. "
                <a href="https://openrouter.ai/keys" target="_blank" rel="noreferrer">"Get one here"</a>
                ". The key is stored in your OS keychain; it never leaves this device."
            </p>
            <input
                type="password"
                placeholder="sk-or-..."
                prop:value=move || val.get()
                on:input=move |ev| val.set(event_target_value(&ev))
                style="padding:.5rem;"
            />
            <button
                on:click=move |_| {
                    let v = val.get().trim().to_string();
                    if !v.is_empty() { on_save(v); }
                }
                disabled=move || val.get().trim().is_empty()
            >
                "Save"
            </button>
        </div>
    }
}
