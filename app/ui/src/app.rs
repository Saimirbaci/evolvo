//! The NewApp content area.
//!
//! **Iteration authors edit this file.** `app.rs` is the inside of the
//! Evolvo shell — it's where the app the user is describing actually gets
//! built. The surrounding shell (app bar, Lineage page, "Star Us" link,
//! Feedback FAB, Canvas overlay, feedback panel) lives in `shell.rs` and is
//! **invariant**: every iteration keeps mounting `<Shell>` and putting the
//! NewApp's Home content inside it. That is what guarantees the four
//! product invariants (Feedback Overlay, per-page Canvas overlay, Inbox,
//! Lineage) survive arbitrary rewrites of the inner app.
//!
//! To build a new app:
//! - Replace `HomePage` below with your app's root component / router /
//!   layout. Add more modules alongside `app.rs` as needed.
//! - Do **not** remove `<Shell>` or move the FAB / Lineage page / Star Us
//!   link into this file — they belong to the shell and must stay there.
//! - Every page/route of your new app is automatically annotatable: the
//!   Canvas overlay mounts on top of whatever you render here when the
//!   user clicks the Feedback FAB.
//! - If you need to react to the Canvas being open (e.g. hide copy from
//!   the page screenshot), read `PanelOpen` from context.

use leptos::prelude::*;

use crate::shell::{PanelOpen, Shell};

#[component]
pub fn App() -> impl IntoView {
    view! {
        <Shell>
            <HomePage />
        </Shell>
    }
}

#[component]
fn HomePage() -> impl IntoView {
    let PanelOpen(panel_open) =
        use_context::<PanelOpen>().expect("Shell must provide PanelOpen context");

    // Remove the welcome copy from the DOM entirely while the canvas overlay
    // is open, so it can't end up in the page screenshot captured at submit.
    view! {
        {move || {
            if panel_open.get() {
                view! { <div class="home-page home-page-empty"></div> }.into_any()
            } else {
                view! {
                    <div class="home-page">
                        <div class="home-hero">
                            <h1 class="home-title">"Welcome to Evolvo"</h1>
                            <p class="home-subtitle">
                                "Click the ✎ button in the bottom-right corner of any page \
                                 to open the Canvas overlay and send feedback about what \
                                 you're looking at."
                            </p>
                            <ul class="home-tips">
                                <li>"Draw, type, or paste screenshots directly on the page."</li>
                                <li>"Record a voice note to add context."</li>
                                <li>"Submit to kick off a lineage iteration."</li>
                            </ul>
                        </div>
                    </div>
                }.into_any()
            }
        }}
    }
}
