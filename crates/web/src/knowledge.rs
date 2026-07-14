//! Knowledge-base browse/search panel (issue #19): search + browse the calling
//! user's long-term knowledge base and open an entry.
//!
//! **Client-facing command surface (scoped).** Unlike the tool-only KB *writes*
//! the assistant performs, the daemon exposes first-class client commands for
//! reading the KB — [`Command::ListKnowledgeEntries`] (browse, newest first),
//! [`Command::SearchKnowledgeEntries`] (hybrid search), and
//! [`Command::GetKnowledgeEntry`] (fetch one by id) — each answering with
//! [`CommandResult::KnowledgeEntries`] / [`CommandResult::KnowledgeEntry`] of
//! wire-clean [`KnowledgeEntryView`]s. The BFF blind-forwards them, so this
//! panel is a real browse/search surface, not a placeholder.
//!
//! **Load-on-demand, not reducer-driven.** The shared `client-ui-common` reducer
//! doesn't model KB state, so — like the connections/purposes/personality panels
//! — the engine loads results straight into view signals (`refresh_knowledge` /
//! `search_knowledge`) rather than routing through `UiMessage`s. A Refresh button
//! re-reads on demand. (Live push via `KnowledgeChanged` is *not* wired: the BFF
//! relay currently drops that background signal, and it carries no conversation
//! to route on — see the PR's deferred-scope note.)
//!
//! **Split like `scratchpad.rs`/`model.rs`.** The pure summary/snippet/date
//! helpers live at module top and unit-test on the host target; the Leptos panel
//! is a `#[cfg(target_arch = "wasm32")]` submodule that consumes *these* helpers,
//! so the tested logic and the rendered logic can't drift.
//!
//! [`Command::ListKnowledgeEntries`]: desktop_assistant_api_model::Command::ListKnowledgeEntries
//! [`Command::SearchKnowledgeEntries`]: desktop_assistant_api_model::Command::SearchKnowledgeEntries
//! [`Command::GetKnowledgeEntry`]: desktop_assistant_api_model::Command::GetKnowledgeEntry
//! [`CommandResult::KnowledgeEntries`]: desktop_assistant_api_model::CommandResult::KnowledgeEntries
//! [`CommandResult::KnowledgeEntry`]: desktop_assistant_api_model::CommandResult::KnowledgeEntry
//! [`KnowledgeEntryView`]: desktop_assistant_api_model::KnowledgeEntryView

/// The empty-knowledge-base line, shared by [`results_summary`] and the panel's
/// browse empty state so the two never drift.
pub const EMPTY_BROWSE: &str =
    "Your knowledge base is empty — Adele saves durable facts here as she learns them.";

/// Collapse an entry's (assistant-produced, possibly multi-line) content into a
/// single-line preview of at most `max_chars` characters, appending an ellipsis
/// when it was truncated. Whitespace runs — including newlines — collapse to one
/// space; empty content yields an empty string. Character-based (not byte-based)
/// so multi-byte content never splits a codepoint.
pub fn snippet(content: &str, max_chars: usize) -> String {
    // STUB (spec commit): the real impl collapses whitespace and truncates; this
    // returns the raw content so the collapse/truncate tests fail red.
    let _ = max_chars;
    content.to_string()
}

/// A one-line summary for the panel header: entry/result counts (singular/
/// plural), branching on whether the current view is a search or a browse. An
/// empty browse returns the inviting [`EMPTY_BROWSE`] line; an empty search
/// returns a plain "No matches."
pub fn results_summary(count: usize, searching: bool) -> String {
    // STUB (spec commit): real impl formats counts; this returns an empty string
    // so every summary assertion fails red.
    let _ = (count, searching);
    String::new()
}

/// Normalize the search box: trim surrounding whitespace and treat an
/// all-whitespace / empty box as "no query" (browse mode), returning `None`.
/// A non-empty query returns its trimmed form.
pub fn normalize_query(raw: &str) -> Option<String> {
    // STUB (spec commit): real impl trims and empties to `None`; this always
    // returns `None` so the non-empty-query tests fail red.
    let _ = raw;
    None
}

/// The date portion of a daemon timestamp for the entry's meta line: everything
/// before the first `T` or space (so `"2026-07-14 00:00:00"` and
/// `"2026-07-14T12:30:00Z"` both render `"2026-07-14"`). A value without either
/// separator is returned unchanged.
pub fn short_date(ts: &str) -> String {
    // STUB (spec commit): real impl splits on the date/time separator; this
    // returns an empty string so the format tests fail red.
    let _ = ts;
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_empty_is_empty() {
        assert_eq!(snippet("", 140), "");
        assert_eq!(
            snippet("   \n  \t ", 140),
            "",
            "all-whitespace collapses away"
        );
    }

    #[test]
    fn snippet_short_content_is_returned_verbatim() {
        assert_eq!(snippet("A short fact.", 140), "A short fact.");
    }

    #[test]
    fn snippet_collapses_internal_whitespace_and_newlines() {
        let content = "Line one.\n\nLine two   has\tgaps.";
        assert_eq!(snippet(content, 140), "Line one. Line two has gaps.");
    }

    #[test]
    fn snippet_truncates_long_content_with_ellipsis() {
        let content = "x".repeat(300);
        let s = snippet(&content, 140);
        assert!(
            s.ends_with('\u{2026}'),
            "truncated snippet ends with an ellipsis: {s:?}"
        );
        // 140 kept chars + the ellipsis = 141 chars.
        assert_eq!(s.chars().count(), 141, "keeps max_chars then one ellipsis");
    }

    #[test]
    fn snippet_exactly_max_is_not_truncated() {
        let content = "y".repeat(140);
        let s = snippet(&content, 140);
        assert_eq!(s, content, "content exactly at the limit is untouched");
        assert!(!s.ends_with('\u{2026}'));
    }

    #[test]
    fn snippet_counts_characters_not_bytes() {
        // Multi-byte content must never split a codepoint and must count by char.
        let content = "é".repeat(10);
        let s = snippet(&content, 5);
        assert_eq!(s.chars().count(), 6, "5 chars + ellipsis");
        assert!(
            s.starts_with("éé"),
            "kept leading multibyte chars intact: {s:?}"
        );
    }

    #[test]
    fn results_summary_browse_empty_invites() {
        assert_eq!(results_summary(0, false), EMPTY_BROWSE);
    }

    #[test]
    fn results_summary_browse_counts_singular_and_plural() {
        assert_eq!(results_summary(1, false), "1 entry");
        assert_eq!(results_summary(7, false), "7 entries");
    }

    #[test]
    fn results_summary_search_empty_reports_no_matches() {
        assert_eq!(results_summary(0, true), "No matches.");
    }

    #[test]
    fn results_summary_search_counts_singular_and_plural() {
        assert_eq!(results_summary(1, true), "1 result");
        assert_eq!(results_summary(4, true), "4 results");
    }

    #[test]
    fn normalize_query_trims_and_keeps_nonempty() {
        assert_eq!(normalize_query("  rust  "), Some("rust".to_string()));
        assert_eq!(
            normalize_query("multi word query"),
            Some("multi word query".to_string())
        );
    }

    #[test]
    fn normalize_query_empty_or_whitespace_is_none() {
        assert_eq!(normalize_query(""), None);
        assert_eq!(
            normalize_query("   \t\n "),
            None,
            "all-whitespace is browse mode"
        );
    }

    #[test]
    fn short_date_extracts_the_date_part() {
        assert_eq!(short_date("2026-07-14 00:00:00"), "2026-07-14");
        assert_eq!(short_date("2026-07-14T12:30:00Z"), "2026-07-14");
    }

    #[test]
    fn short_date_without_separator_is_unchanged() {
        assert_eq!(short_date("2026-07-14"), "2026-07-14");
        assert_eq!(short_date(""), "");
        assert_eq!(short_date("unknown"), "unknown");
    }
}
