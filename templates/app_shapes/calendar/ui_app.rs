//! calendar — UI scaffold (the NewApp content area).
//!
//! Drop into `app/ui/src/app_shape_calendar.rs`, declare the new module
//! in `app/ui/src/main.rs`, and rewrite `app/ui/src/app.rs` so its `App`
//! component wraps `<CalendarPage/>` inside `<Shell>`.

use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;

use crate::interop;
use crate::types::CalendarEvent;

#[component]
pub fn CalendarPage() -> impl IntoView {
    // Anchor date determines the visible month. Stored as (year, month0)
    // where month0 is 0-11 — matches JS Date semantics, which we use to
    // compute cell dates without pulling chrono into WASM.
    let now = js_sys::Date::new_0();
    let year: RwSignal<i32> = RwSignal::new(now.get_full_year() as i32);
    let month0: RwSignal<u32> = RwSignal::new(now.get_month());
    let events: RwSignal<Vec<CalendarEvent>> = RwSignal::new(Vec::new());
    let load_error: RwSignal<Option<String>> = RwSignal::new(None);
    let selected_day: RwSignal<Option<(i32, u32, u32)>> = RwSignal::new(None);

    // Selected event being edited (None = closed; Some = panel open).
    let editor_open: RwSignal<bool> = RwSignal::new(false);
    let editor_id: RwSignal<Option<String>> = RwSignal::new(None);
    let editor_title: RwSignal<String> = RwSignal::new(String::new());
    let editor_all_day: RwSignal<bool> = RwSignal::new(true);
    let editor_start: RwSignal<String> = RwSignal::new(String::new()); // YYYY-MM-DD
    let editor_end: RwSignal<String> = RwSignal::new(String::new());
    let editor_notes: RwSignal<String> = RwSignal::new(String::new());

    let reload: RwSignal<u32> = RwSignal::new(0);
    Effect::new(move |_| {
        let _ = reload.get();
        let y = year.get();
        let m = month0.get();
        let start = month_start_ms(y, m);
        let end = month_start_ms(y + ((m as i32 + 1) / 12), (m + 1) % 12);
        spawn_local(async move {
            match interop::list_events_in_range(start, end).await {
                Ok(es) => events.set(es),
                Err(e) => load_error.set(Some(e)),
            }
        });
    });

    let prev_month = move || {
        let m = month0.get_untracked();
        if m == 0 {
            year.update(|y| *y -= 1);
            month0.set(11);
        } else {
            month0.update(|m| *m -= 1);
        }
    };
    let next_month = move || {
        let m = month0.get_untracked();
        if m == 11 {
            year.update(|y| *y += 1);
            month0.set(0);
        } else {
            month0.update(|m| *m += 1);
        }
    };
    let go_today = move || {
        let now = js_sys::Date::new_0();
        year.set(now.get_full_year() as i32);
        month0.set(now.get_month());
    };

    let open_new = move |day: u32| {
        let y = year.get_untracked();
        let m = month0.get_untracked();
        editor_id.set(None);
        editor_title.set(String::new());
        editor_all_day.set(true);
        editor_start.set(format_ymd(y, m, day));
        editor_end.set(format_ymd(y, m, day));
        editor_notes.set(String::new());
        editor_open.set(true);
    };

    let open_edit = move |event: CalendarEvent| {
        editor_id.set(Some(event.id.clone()));
        editor_title.set(event.title.clone());
        editor_all_day.set(event.all_day);
        editor_start.set(ms_to_ymd(event.start_ms));
        editor_end.set(ms_to_ymd(event.end_ms));
        editor_notes.set(event.notes.clone().unwrap_or_default());
        editor_open.set(true);
    };

    let save = move || {
        let title = editor_title.get_untracked().trim().to_string();
        if title.is_empty() {
            return;
        }
        let start_ms = ymd_to_ms(&editor_start.get_untracked());
        let end_ms = ymd_to_ms(&editor_end.get_untracked()).max(start_ms);
        let id = editor_id.get_untracked();
        let all_day = editor_all_day.get_untracked();
        let notes = editor_notes.get_untracked().trim().to_string();
        let notes_opt = if notes.is_empty() { None } else { Some(notes) };
        spawn_local(async move {
            match interop::save_event(
                id.as_deref(),
                &title,
                start_ms,
                end_ms,
                all_day,
                notes_opt.as_deref(),
            )
            .await
            {
                Ok(_) => {
                    editor_open.set(false);
                    reload.update(|v| *v = v.wrapping_add(1));
                }
                Err(e) => load_error.set(Some(e)),
            }
        });
    };

    let do_delete = move || {
        let Some(id) = editor_id.get_untracked() else {
            return;
        };
        spawn_local(async move {
            match interop::delete_event(&id).await {
                Ok(_) => {
                    editor_open.set(false);
                    reload.update(|v| *v = v.wrapping_add(1));
                }
                Err(e) => load_error.set(Some(e)),
            }
        });
    };

    view! {
        <div class="cal-page">
            <header class="cal-toolbar">
                <button class="secondary-btn" on:click=move |_| prev_month.clone()() aria-label="Previous month" title="Previous month">"‹"</button>
                <button class="secondary-btn" on:click=move |_| go_today.clone()() aria-label="Go to today" title="Today">"Today"</button>
                <button class="secondary-btn" on:click=move |_| next_month.clone()() aria-label="Next month" title="Next month">"›"</button>
                <h2 class="cal-month-title">{move || format_month(year.get(), month0.get())}</h2>
            </header>

            {move || match load_error.get() {
                Some(e) => view! {
                    <div class="cal-error" role="alert">{e}</div>
                }.into_any(),
                None => view! { <span></span> }.into_any(),
            }}

            <section class="cal-grid">
                <div class="cal-grid-head">
                    {["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"].iter().map(|n| view! {
                        <span class="cal-dow">{n.to_string()}</span>
                    }).collect_view()}
                </div>
                {move || {
                    let y = year.get();
                    let m = month0.get();
                    let cells = month_cells(y, m);
                    let evs = events.get();
                    cells.into_iter().map(|cell| {
                        let in_month = cell.month == m;
                        let day_events: Vec<CalendarEvent> = evs.iter()
                            .filter(|e| ms_falls_on(e.start_ms, e.end_ms, cell.year, cell.month, cell.day))
                            .cloned()
                            .collect();
                        let day = cell.day;
                        let cell_year = cell.year;
                        let cell_month = cell.month;
                        let is_selected = move || selected_day.get() == Some((cell_year, cell_month, day));
                        let is_today = is_today(cell_year, cell_month, day);
                        let mut classes = String::from("cal-cell");
                        if !in_month {
                            classes.push_str(" cal-cell-other");
                        }
                        if is_today {
                            classes.push_str(" cal-cell-today");
                        }
                        let open_new = open_new.clone();
                        let open_edit = open_edit.clone();
                        view! {
                            <div
                                class=classes
                                class:active=is_selected
                                on:click=move |_| {
                                    selected_day.set(Some((cell_year, cell_month, day)));
                                }
                                on:dblclick=move |_| open_new.clone()(day)
                            >
                                <header class="cal-cell-head">
                                    <span class="cal-cell-day">{day}</span>
                                    <button
                                        class="cal-cell-add"
                                        on:click=move |ev| {
                                            ev.stop_propagation();
                                            open_new.clone()(day);
                                        }
                                        aria-label="Add event"
                                        title="Add event on this day"
                                    >
                                        "+"
                                    </button>
                                </header>
                                <ul class="cal-cell-events">
                                    {day_events.into_iter().map(|ev| {
                                        let title = ev.title.clone();
                                        let all_day = ev.all_day;
                                        let label = if all_day {
                                            title.clone()
                                        } else {
                                            format!("{} {}", format_hm(ev.start_ms), title)
                                        };
                                        let ev_for_click = ev.clone();
                                        let open_edit = open_edit.clone();
                                        view! {
                                            <li
                                                class="cal-event"
                                                on:click=move |e| {
                                                    e.stop_propagation();
                                                    open_edit(ev_for_click.clone());
                                                }
                                            >
                                                // FILL: event categories — render a colored
                                                // pill / icon based on `event.category`.
                                                {label}
                                            </li>
                                        }
                                    }).collect_view()}
                                </ul>
                            </div>
                        }
                    }).collect_view()
                }}
            </section>

            // FILL: week view — alternate rendering tab.
            // FILL: agenda view — alternate rendering tab.
            // FILL: timezone handling — surface a tz picker.

            {move || {
                if !editor_open.get() {
                    return view! { <span></span> }.into_any();
                }
                let save = save.clone();
                let do_delete = do_delete.clone();
                view! {
                    <div class="cal-editor-overlay" on:click=move |_| editor_open.set(false)>
                        <div class="cal-editor" on:click=move |ev| ev.stop_propagation()>
                            <header><h3>{move || if editor_id.get().is_some() { "Edit event" } else { "New event" }}</h3></header>
                            <label class="cal-editor-field">
                                <span>"Title"</span>
                                <input
                                    type="text"
                                    prop:value=move || editor_title.get()
                                    on:input=move |ev| {
                                        let t = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                        if let Some(t) = t { editor_title.set(t.value()); }
                                    }
                                />
                            </label>
                            <label class="cal-editor-field cal-editor-checkbox">
                                <input
                                    type="checkbox"
                                    prop:checked=move || editor_all_day.get()
                                    on:change=move |ev| {
                                        let t = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                        if let Some(t) = t { editor_all_day.set(t.checked()); }
                                    }
                                />
                                <span>"All day"</span>
                            </label>
                            <label class="cal-editor-field">
                                <span>"Start"</span>
                                <input
                                    type="date"
                                    prop:value=move || editor_start.get()
                                    on:change=move |ev| {
                                        let t = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                        if let Some(t) = t { editor_start.set(t.value()); }
                                    }
                                />
                            </label>
                            <label class="cal-editor-field">
                                <span>"End"</span>
                                <input
                                    type="date"
                                    prop:value=move || editor_end.get()
                                    on:change=move |ev| {
                                        let t = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                        if let Some(t) = t { editor_end.set(t.value()); }
                                    }
                                />
                            </label>
                            <label class="cal-editor-field">
                                <span>"Notes"</span>
                                <textarea
                                    prop:value=move || editor_notes.get()
                                    on:input=move |ev| {
                                        let t = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlTextAreaElement>().ok());
                                        if let Some(t) = t { editor_notes.set(t.value()); }
                                    }
                                ></textarea>
                            </label>
                            <footer class="cal-editor-actions">
                                {move || {
                                    if editor_id.get().is_some() {
                                        let do_delete = do_delete.clone();
                                        view! {
                                            <button class="secondary-btn" on:click=move |_| do_delete()>"Delete"</button>
                                        }.into_any()
                                    } else {
                                        view! { <span></span> }.into_any()
                                    }
                                }}
                                <button class="secondary-btn" on:click=move |_| editor_open.set(false)>"Cancel"</button>
                                <button class="primary-btn" on:click=move |_| save.clone()()>"Save"</button>
                            </footer>
                        </div>
                    </div>
                }.into_any()
            }}
        </div>
    }
}

#[derive(Clone, Copy, Debug)]
struct DayCell {
    year: i32,
    month: u32,
    day: u32,
}

fn month_cells(year: i32, month0: u32) -> Vec<DayCell> {
    // Build a 6x7 = 42 cell month grid starting from the Sunday on or
    // before the first of the month and ending after we've covered every
    // day of the month plus enough trailing days to fill the last row.
    let first = js_sys::Date::new_with_year_month_day(year as u32, month0 as i32, 1);
    let dow = first.get_day(); // 0 = Sun
    let mut cells = Vec::with_capacity(42);
    for i in 0..42 {
        let offset = (i as i32) - (dow as i32);
        let d = js_sys::Date::new_with_year_month_day(year as u32, month0 as i32, 1);
        d.set_date(1 + offset);
        cells.push(DayCell {
            year: d.get_full_year() as i32,
            month: d.get_month(),
            day: d.get_date(),
        });
    }
    cells
}

fn month_start_ms(year: i32, month0: u32) -> u64 {
    let d = js_sys::Date::new_with_year_month_day(year as u32, month0 as i32, 1);
    d.get_time() as u64
}

fn ms_falls_on(start_ms: u64, end_ms: u64, year: i32, month0: u32, day: u32) -> bool {
    let day_start = js_sys::Date::new_with_year_month_day(year as u32, month0 as i32, day as i32);
    let day_start_ms = day_start.get_time() as u64;
    let day_end_ms = day_start_ms + 24 * 60 * 60 * 1000;
    start_ms < day_end_ms && end_ms > day_start_ms
}

fn is_today(year: i32, month0: u32, day: u32) -> bool {
    let now = js_sys::Date::new_0();
    now.get_full_year() as i32 == year && now.get_month() == month0 && now.get_date() == day
}

fn format_month(year: i32, month0: u32) -> String {
    const NAMES: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    let name = NAMES.get(month0 as usize).copied().unwrap_or("");
    format!("{name} {year}")
}

fn format_ymd(year: i32, month0: u32, day: u32) -> String {
    format!("{:04}-{:02}-{:02}", year, month0 + 1, day)
}

fn format_hm(ms: u64) -> String {
    if ms == 0 {
        return String::new();
    }
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms as f64));
    let s: String = date.to_iso_string().into();
    if s.len() >= 16 {
        s[11..16].to_string()
    } else {
        s
    }
}

fn ms_to_ymd(ms: u64) -> String {
    if ms == 0 {
        return String::new();
    }
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms as f64));
    format!(
        "{:04}-{:02}-{:02}",
        d.get_full_year(),
        d.get_month() + 1,
        d.get_date()
    )
}

fn ymd_to_ms(s: &str) -> u64 {
    if s.len() < 10 {
        return 0;
    }
    let y: i32 = s[0..4].parse().unwrap_or(1970);
    let m: u32 = s[5..7].parse().unwrap_or(1);
    let d: u32 = s[8..10].parse().unwrap_or(1);
    let date = js_sys::Date::new_with_year_month_day(y as u32, (m as i32) - 1, d as i32);
    date.get_time() as u64
}
