//! chat_app — UI scaffold (the NewApp content area).
//!
//! Drop into `app/ui/src/app_shape_chat_app.rs`, declare the new module
//! in `app/ui/src/main.rs`, and rewrite `app/ui/src/app.rs` so its `App`
//! component wraps `<ChatAppPage/>` inside `<Shell>`.

use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;

use crate::interop;
use crate::types::Conversation;

#[component]
pub fn ChatAppPage() -> impl IntoView {
    let conversations: RwSignal<Vec<Conversation>> = RwSignal::new(Vec::new());
    let active: RwSignal<Option<Conversation>> = RwSignal::new(None);
    let composer: RwSignal<String> = RwSignal::new(String::new());
    let load_error: RwSignal<Option<String>> = RwSignal::new(None);
    let sending: RwSignal<bool> = RwSignal::new(false);

    let reload: RwSignal<u32> = RwSignal::new(0);
    Effect::new(move |_| {
        let _ = reload.get();
        spawn_local(async move {
            match interop::list_conversations().await {
                Ok(list) => conversations.set(list),
                Err(e) => load_error.set(Some(e)),
            }
        });
    });

    let select_conv = move |id: String| {
        spawn_local(async move {
            match interop::load_conversation(&id).await {
                Ok(c) => active.set(Some(c)),
                Err(e) => load_error.set(Some(e)),
            }
        });
    };

    let new_conv = move || {
        spawn_local(async move {
            match interop::save_conversation(None, "New conversation").await {
                Ok(c) => {
                    active.set(Some(c));
                    reload.update(|v| *v = v.wrapping_add(1));
                }
                Err(e) => load_error.set(Some(e)),
            }
        });
    };

    let do_delete = move |id: String| {
        spawn_local(async move {
            match interop::delete_conversation(&id).await {
                Ok(_) => {
                    if active.get_untracked().as_ref().map(|c| c.id.clone()) == Some(id.clone()) {
                        active.set(None);
                    }
                    reload.update(|v| *v = v.wrapping_add(1));
                }
                Err(e) => load_error.set(Some(e)),
            }
        });
    };

    let send = move || {
        if sending.get_untracked() {
            return;
        }
        let Some(conv) = active.get_untracked() else {
            return;
        };
        let content = composer.get_untracked().trim().to_string();
        if content.is_empty() {
            return;
        }
        sending.set(true);
        composer.set(String::new());
        spawn_local(async move {
            match interop::send_message(&conv.id, &content).await {
                Ok(updated) => {
                    active.set(Some(updated));
                    reload.update(|v| *v = v.wrapping_add(1));
                }
                Err(e) => load_error.set(Some(e)),
            }
            sending.set(false);
        });
    };

    // FILL: keyboard shortcuts — ⌘⏎ to send, ⌘N for new conversation, etc.

    view! {
        <div class="chat-page">
            <aside class="chat-sidebar">
                <header class="chat-sidebar-head">
                    <h2 class="chat-sidebar-title">"Conversations"</h2>
                    <button
                        class="primary-btn"
                        on:click=move |_| new_conv.clone()()
                        aria-label="New conversation"
                        title="Start a new conversation"
                    >
                        "+ New"
                    </button>
                </header>
                {move || {
                    let convs = conversations.get();
                    if convs.is_empty() {
                        return view! {
                            <div class="chat-empty">"No conversations yet."</div>
                        }.into_any();
                    }
                    view! {
                        <ul class="chat-conv-list">
                            {convs.into_iter().map(|c| {
                                let id = c.id.clone();
                                let id_for_click = id.clone();
                                let id_for_delete = id.clone();
                                let id_for_active = id.clone();
                                let title = c.title.clone();
                                let is_active = move || {
                                    active.get().as_ref().map(|a| a.id.clone()).as_deref()
                                        == Some(id_for_active.as_str())
                                };
                                view! {
                                    <li>
                                        <button
                                            class="chat-conv-item"
                                            class:active=is_active
                                            on:click=move |_| select_conv.clone()(id_for_click.clone())
                                        >
                                            <span class="chat-conv-title">{title}</span>
                                            <button
                                                class="chat-conv-delete"
                                                on:click=move |ev| {
                                                    ev.stop_propagation();
                                                    do_delete.clone()(id_for_delete.clone());
                                                }
                                                aria-label="Delete conversation"
                                                title="Delete this conversation"
                                            >
                                                "×"
                                            </button>
                                        </button>
                                    </li>
                                }
                            }).collect_view()}
                        </ul>
                    }.into_any()
                }}
            </aside>

            <section class="chat-main">
                {move || match load_error.get() {
                    Some(e) => view! {
                        <div class="chat-error" role="alert">{e}</div>
                    }.into_any(),
                    None => view! { <span></span> }.into_any(),
                }}

                {move || match active.get() {
                    None => view! {
                        <div class="chat-empty chat-empty-large">
                            "Pick a conversation or start a new one."
                        </div>
                    }.into_any(),
                    Some(conv) => {
                        let title = conv.title.clone();
                        let messages = conv.messages.clone();
                        view! {
                            <header class="chat-main-head">
                                <h3>{title}</h3>
                            </header>
                            <ul class="chat-stream">
                                // FILL: streaming UI affordance — when bound
                                // to openrouter with streaming, render a
                                // pulsing cursor while the assistant token
                                // stream is in flight.
                                {messages.into_iter().map(|m| {
                                    let role_class = format!("chat-msg chat-msg-{}", m.role);
                                    view! {
                                        <li class=role_class>
                                            <span class="chat-msg-role">{m.role.clone()}</span>
                                            <span class="chat-msg-body">{m.content}</span>
                                        </li>
                                    }
                                }).collect_view()}
                            </ul>
                        }.into_any()
                    }
                }}

                <footer class="chat-composer">
                    <textarea
                        class="chat-composer-input"
                        placeholder="Type a message…"
                        prop:value=move || composer.get()
                        on:input=move |ev| {
                            let t = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlTextAreaElement>().ok());
                            if let Some(t) = t { composer.set(t.value()); }
                        }
                        on:keydown=move |ev| {
                            if ev.key() == "Enter" && !ev.shift_key() {
                                ev.prevent_default();
                                send.clone()();
                            }
                        }
                        prop:disabled=move || active.get().is_none() || sending.get()
                        aria-label="Message"
                    ></textarea>
                    <button
                        class="primary-btn"
                        on:click=move |_| send.clone()()
                        prop:disabled=move || active.get().is_none() || sending.get() || composer.get().trim().is_empty()
                        aria-label="Send message"
                        title="Send (Enter; Shift+Enter for newline)"
                    >
                        {move || if sending.get() { "Sending…" } else { "Send" }}
                    </button>
                </footer>
            </section>
        </div>
    }
}
