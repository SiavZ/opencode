//! Purpose: Markdown to sanitized HTML (pulldown-cmark + ammonia), syntax highlighting (syntect), streaming-safe incremental rendering.
//! Scope: pure render functions, native-testable.
//! Uses:
//! `-- none
//! External:
//! |-- ammonia::clean
//! |-- pulldown_cmark::Parser::new
//! `-- pulldown_cmark::html::push_html
//! Used by:
//! rust/crates/
//! `-- ui-web/src/lib.rs
//! Invariants: skeleton owned by child node p10.render; replace this header when implementing.
//! Contract tests: ui-web/tests (owner p10.render)

/// Render untrusted Markdown into sanitized HTML.
pub fn render(source: &str) -> String {
    let mut out = String::new();
    pulldown_cmark::html::push_html(&mut out, pulldown_cmark::Parser::new(source));
    ammonia::clean(&out)
}
