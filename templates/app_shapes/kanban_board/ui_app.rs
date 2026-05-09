//! kanban_board — UI scaffold (the NewApp content area).
//!
//! Drop into `app/ui/src/app_shape_kanban_board.rs`, declare the new
//! module in `app/ui/src/main.rs`, and rewrite `app/ui/src/app.rs` so its
//! `App` component wraps `<KanbanBoardPage/>` inside `<Shell>`.

use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;

use crate::interop;
use crate::types::{BoardState, Card};

#[component]
pub fn KanbanBoardPage() -> impl IntoView {
    let board: RwSignal<Option<BoardState>> = RwSignal::new(None);
    let load_error: RwSignal<Option<String>> = RwSignal::new(None);
    // Card currently being dragged; identifies the source column too. Set
    // on `dragstart`, cleared on `dragend` / drop.
    let dragging: RwSignal<Option<(String, String)>> = RwSignal::new(None);
    let new_title: RwSignal<String> = RwSignal::new(String::new());
    let new_column_id: RwSignal<String> = RwSignal::new(String::new());

    Effect::new(move |once: Option<()>| {
        if once.is_some() {
            return;
        }
        spawn_local(async move {
            match interop::load_board().await {
                Ok(b) => {
                    if new_column_id.get_untracked().is_empty() {
                        if let Some(first) = b.columns.first() {
                            new_column_id.set(first.id.clone());
                        }
                    }
                    board.set(Some(b));
                }
                Err(e) => load_error.set(Some(e)),
            }
        });
    });

    let persist = move |b: BoardState| {
        spawn_local(async move {
            match interop::save_board(b).await {
                Ok(saved) => board.set(Some(saved)),
                Err(e) => load_error.set(Some(e)),
            }
        });
    };

    let move_card = move |card_id: String, target_column_id: String| {
        let Some(mut b) = board.get_untracked() else {
            return;
        };
        let mut moved: Option<Card> = None;
        for col in b.columns.iter_mut() {
            if let Some(idx) = col.cards.iter().position(|c| c.id == card_id) {
                moved = Some(col.cards.remove(idx));
                break;
            }
        }
        let Some(mut card) = moved else { return };
        card.column_id = target_column_id.clone();
        if let Some(target) = b.columns.iter_mut().find(|c| c.id == target_column_id) {
            card.ordinal = target.cards.len() as u32;
            target.cards.push(card);
        }
        persist.clone()(b);
    };

    let add_card = move || {
        let title = new_title.get_untracked().trim().to_string();
        let column_id = new_column_id.get_untracked();
        if title.is_empty() || column_id.is_empty() {
            return;
        }
        let Some(mut b) = board.get_untracked() else {
            return;
        };
        let id = format!("card-{}", js_sys::Date::now() as u64);
        let new_card = Card {
            id,
            title,
            body: String::new(),
            column_id: column_id.clone(),
            ordinal: 0,
            updated_at: 0,
        };
        if let Some(target) = b.columns.iter_mut().find(|c| c.id == column_id) {
            let mut card = new_card;
            card.ordinal = target.cards.len() as u32;
            target.cards.push(card);
        }
        new_title.set(String::new());
        persist.clone()(b);
    };

    let delete_card = move |card_id: String| {
        let Some(mut b) = board.get_untracked() else {
            return;
        };
        for col in b.columns.iter_mut() {
            col.cards.retain(|c| c.id != card_id);
        }
        persist.clone()(b);
    };

    // FILL: keyboard shortcuts — bind 'n' to focus the new-card input,
    // arrow keys to move the highlighted card between columns, etc.

    view! {
        <div class="kanban-page">
            <header class="kanban-toolbar">
                <input
                    class="kanban-new-title"
                    placeholder="New card title"
                    type="text"
                    prop:value=move || new_title.get()
                    on:input=move |ev| {
                        let t = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                        if let Some(t) = t { new_title.set(t.value()); }
                    }
                    on:keydown=move |ev| {
                        if ev.key() == "Enter" {
                            ev.prevent_default();
                            add_card.clone()();
                        }
                    }
                    aria-label="New card title"
                />
                <select
                    class="kanban-new-column"
                    prop:value=move || new_column_id.get()
                    on:change=move |ev| {
                        let t = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlSelectElement>().ok());
                        if let Some(t) = t { new_column_id.set(t.value()); }
                    }
                    aria-label="Target column"
                >
                    {move || {
                        let cols = board.get().map(|b| b.columns.clone()).unwrap_or_default();
                        cols.into_iter().map(|c| {
                            let id = c.id.clone();
                            let name = c.name.clone();
                            view! { <option value=id>{name}</option> }
                        }).collect_view()
                    }}
                </select>
                <button
                    class="primary-btn"
                    on:click=move |_| add_card.clone()()
                    prop:disabled=move || new_title.get().trim().is_empty()
                    aria-label="Add card"
                    title="Add a new card to the selected column"
                >
                    "+ Add"
                </button>
            </header>

            {move || match load_error.get() {
                Some(e) => view! {
                    <div class="kanban-error" role="alert">{e}</div>
                }.into_any(),
                None => view! { <span></span> }.into_any(),
            }}

            <div class="kanban-columns">
                {move || {
                    let Some(b) = board.get() else {
                        return view! { <div class="empty-state">"Loading…"</div> }.into_any();
                    };
                    b.columns.into_iter().map(|col| {
                        let column_id = col.id.clone();
                        let column_id_for_drop = column_id.clone();
                        let column_id_for_drop_zone = column_id.clone();
                        view! {
                            <section
                                class="kanban-column"
                                on:dragover=move |ev| { ev.prevent_default(); }
                                on:drop=move |ev| {
                                    ev.prevent_default();
                                    if let Some((card_id, _src_col)) = dragging.get_untracked() {
                                        move_card.clone()(card_id, column_id_for_drop.clone());
                                    }
                                    dragging.set(None);
                                }
                            >
                                <header class="kanban-column-head">
                                    <h3>{col.name.clone()}</h3>
                                    <span class="kanban-column-count">{col.cards.len()}</span>
                                </header>
                                <ul class="kanban-card-list">
                                    {col.cards.into_iter().map(|card| {
                                        let card_id = card.id.clone();
                                        let card_id_for_dragstart = card_id.clone();
                                        let card_id_for_delete = card_id.clone();
                                        let src_col = column_id_for_drop_zone.clone();
                                        view! {
                                            <li
                                                class="kanban-card"
                                                draggable="true"
                                                on:dragstart=move |_| {
                                                    dragging.set(Some((card_id_for_dragstart.clone(), src_col.clone())));
                                                }
                                                on:dragend=move |_| { dragging.set(None); }
                                            >
                                                <div class="kanban-card-title">{card.title}</div>
                                                {if card.body.is_empty() {
                                                    view! { <span></span> }.into_any()
                                                } else {
                                                    view! { <div class="kanban-card-body">{card.body}</div> }.into_any()
                                                }}
                                                <button
                                                    class="kanban-card-delete"
                                                    on:click=move |_| delete_card.clone()(card_id_for_delete.clone())
                                                    aria-label="Delete card"
                                                    title="Delete this card"
                                                >
                                                    "×"
                                                </button>
                                            </li>
                                        }
                                    }).collect_view()}
                                </ul>
                            </section>
                        }
                    }).collect_view().into_any()
                }}
            </div>
        </div>
    }
}
