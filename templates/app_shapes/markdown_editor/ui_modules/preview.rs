//! markdown_editor — read-only preview pane.
//!
//! Drop into `app/ui/src/markdown_editor_preview.rs`. Declared as a module
//! in `app/ui/src/main.rs`.
//!
//! The default renderer below is intentionally minimal — it escapes HTML
//! and preserves whitespace, so the preview reads as a "raw view" of the
//! source. To get real rendering, swap the body of `render_markdown` for a
//! call into a richer renderer. Two reasonable upgrades:
//!
//! 1. Run `pulldown-cmark` on the host in a `render_markdown` Tauri
//!    command and call it from the UI with debouncing — keeps the WASM
//!    binary small (the parser stays out of the browser bundle).
//! 2. Add `pulldown-cmark` to the UI crate (~50 KB after wasm-opt) and
//!    parse client-side. Faster feedback, larger bundle.
//!
//! The scaffold leaves this as a FILL because the right tradeoff depends
//! on the user's request (offline-first vs bundle size).

use leptos::prelude::*;

#[component]
pub fn MarkdownPreview(body: RwSignal<String>) -> impl IntoView {
    view! {
        <section class="md-editor-preview" aria-label="Rendered preview">
            <pre class="md-editor-preview-pre">
                {move || render_markdown(&body.get())}
            </pre>
        </section>
    }
}

/// Default "preview": HTML-escape and return the raw markdown so it
/// renders verbatim inside `<pre>`. Replace with a real markdown→HTML
/// renderer for higher fidelity (see module-level docs).
fn render_markdown(source: &str) -> String {
    // FILL: markdown rendering — replace this body with a real renderer.
    // The trivial escape below keeps the preview honest until then: the
    // user sees the source they typed, not an empty pane that pretends to
    // be rendered.
    let mut out = String::with_capacity(source.len());
    for ch in source.chars() {
        match ch {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_html() {
        let s = render_markdown("<script>alert(1)</script> & x");
        assert_eq!(s, "&lt;script&gt;alert(1)&lt;/script&gt; &amp; x");
    }

    #[test]
    fn preserves_whitespace_and_newlines() {
        let s = render_markdown("# Hello\n\n  world");
        assert_eq!(s, "# Hello\n\n  world");
    }
}
