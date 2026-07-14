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

/// How many entries a browse/search reads at once. A single generous page keeps
/// the panel a simple read-only list (no pagination in v1); the daemon caps the
/// KB well below this in practice.
pub const KB_LIMIT: u32 = 50;

/// Characters kept in a collapsed list-row preview before the ellipsis. The full
/// content shows when the row is opened.
const SNIPPET_CHARS: usize = 140;

/// Collapse an entry's (assistant-produced, possibly multi-line) content into a
/// single-line preview of at most `max_chars` characters, appending an ellipsis
/// when it was truncated. Whitespace runs — including newlines — collapse to one
/// space; empty content yields an empty string. Character-based (not byte-based)
/// so multi-byte content never splits a codepoint.
pub fn snippet(content: &str, max_chars: usize) -> String {
    let collapsed = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max_chars {
        return collapsed;
    }
    let kept: String = collapsed.chars().take(max_chars).collect();
    format!("{kept}\u{2026}")
}

/// A one-line summary for the panel header: entry/result counts (singular/
/// plural), branching on whether the current view is a search or a browse. An
/// empty browse returns the inviting [`EMPTY_BROWSE`] line; an empty search
/// returns a plain "No matches."
pub fn results_summary(count: usize, searching: bool) -> String {
    match (searching, count) {
        (false, 0) => EMPTY_BROWSE.to_string(),
        (false, 1) => "1 entry".to_string(),
        (false, n) => format!("{n} entries"),
        (true, 0) => "No matches.".to_string(),
        (true, 1) => "1 result".to_string(),
        (true, n) => format!("{n} results"),
    }
}

/// Normalize the search box: trim surrounding whitespace and treat an
/// all-whitespace / empty box as "no query" (browse mode), returning `None`.
/// A non-empty query returns its trimmed form.
pub fn normalize_query(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// The date portion of a daemon timestamp for the entry's meta line: everything
/// before the first `T` or space (so `"2026-07-14 00:00:00"` and
/// `"2026-07-14T12:30:00Z"` both render `"2026-07-14"`). A value without either
/// separator is returned unchanged.
pub fn short_date(ts: &str) -> String {
    ts.split(['T', ' ']).next().unwrap_or(ts).to_string()
}

/// A short meta line for an entry: when it was added, plus an "Updated" clause
/// only when the update date differs from the creation date (so an untouched
/// entry doesn't show a redundant "Added X · Updated X"). Empty timestamps are
/// tolerated (an empty date simply drops out).
pub fn meta_line(created_at: &str, updated_at: &str) -> String {
    let created = short_date(created_at);
    let updated = short_date(updated_at);
    match (created.is_empty(), updated.is_empty() || updated == created) {
        (true, _) if updated.is_empty() => String::new(),
        (true, _) => format!("Updated {updated}"),
        (false, true) => format!("Added {created}"),
        (false, false) => format!("Added {created} \u{00b7} Updated {updated}"),
    }
}

/// The Leptos knowledge-base panel (issue #19). Re-exported from the wasm-only
/// [`ui`] submodule; `settings.rs` renders it as the `KnowledgeBase` panel body.
#[cfg(target_arch = "wasm32")]
pub use ui::knowledge_panel;

#[cfg(target_arch = "wasm32")]
mod ui {
    //! Mobile-first Leptos view: a search box over a list of KB entries. Each row
    //! is a >=44px tappable accordion header (a single-line snippet + tag chips +
    //! date) that expands to the entry's full content when opened. Browse (most
    //! recent) on open; typing a query and submitting runs a server-side search;
    //! "Clear search" returns to browse. The list is driven by the engine's
    //! `refresh_knowledge` / `search_knowledge` into `view.knowledge_*` signals.
    //!
    //! Read-only: entry content is assistant-produced, so it renders as escaped
    //! plain text (Leptos escapes text children) — never `inner_html`.

    use leptos::prelude::*;

    use desktop_assistant_api_model::KnowledgeEntryView;

    use super::{
        EMPTY_BROWSE, SNIPPET_CHARS, meta_line, normalize_query, results_summary, snippet,
    };
    use crate::engine::ViewSignals;
    use crate::settings::EngineHandle;

    /// The panel body. Loads the browse list on open, hosts the search box, and
    /// renders the results as expandable rows.
    pub fn knowledge_panel(engine: EngineHandle, view: ViewSignals) -> impl IntoView {
        // The search box text.
        let query = RwSignal::new(String::new());
        // Whether the shown results came from a search (drives the summary wording
        // + the Clear affordance). Set synchronously at submit time so the header
        // matches what produced the rows.
        let searching = RwSignal::new(false);
        // The id of the one expanded ("opened") entry, if any.
        let expanded = RwSignal::new(None::<String>);

        // Load the browse list once the panel mounts (re-created each time the tab
        // opens, so this refreshes on every open). Deferred via an effect so the
        // signal writes don't happen during render.
        Effect::new(move |_| {
            engine.with_value(|e| e.borrow().refresh_knowledge());
        });

        let submit = move |ev: leptos::ev::SubmitEvent| {
            ev.prevent_default();
            expanded.set(None);
            match normalize_query(&query.get_untracked()) {
                Some(q) => {
                    searching.set(true);
                    engine.with_value(|e| e.borrow().search_knowledge(q));
                }
                None => {
                    searching.set(false);
                    engine.with_value(|e| e.borrow().refresh_knowledge());
                }
            }
        };

        let clear = move |_| {
            query.set(String::new());
            searching.set(false);
            expanded.set(None);
            engine.with_value(|e| e.borrow().refresh_knowledge());
        };

        view! {
            <section class="panel knowledge-panel">
                <div class="panel-intro">
                    <p class="panel-summary">
                        {move || {
                            results_summary(view.knowledge_entries.get().len(), searching.get())
                        }}
                    </p>
                    <p class="panel-note muted">
                        "Adele's long-term memory \u{2014} durable facts she keeps across \
                         conversations. Search to find one, or browse the most recent. Read-only \
                         here for now."
                    </p>
                </div>

                <form class="kb-search" on:submit=submit>
                    <input
                        class="conn-input"
                        type="search"
                        placeholder="Search the knowledge base\u{2026}"
                        prop:value=move || query.get()
                        on:input=move |ev| query.set(event_target_value(&ev))
                    />
                    <button class="kb-search-btn" type="submit">
                        "Search"
                    </button>
                </form>

                <div class="field-head">
                    <span class="field-label">
                        {move || if searching.get() { "Results" } else { "Recent" }}
                    </span>
                    <Show when=move || searching.get()>
                        <button class="link" on:click=clear>
                            "Clear search"
                        </button>
                    </Show>
                </div>

                {move || {
                    view.knowledge_error
                        .get()
                        .map(|e| {
                            view! {
                                <p class="conn-error" role="alert">
                                    {e}
                                </p>
                            }
                            .into_any()
                        })
                        .unwrap_or_else(|| ().into_any())
                }}

                {move || {
                    let entries = view.knowledge_entries.get();
                    let open = expanded.get();
                    if entries.is_empty() {
                        empty_state(
                            view.knowledge_busy.get(),
                            view.knowledge_loaded.get(),
                            searching.get(),
                        )
                    } else {
                        entries
                            .into_iter()
                            .map(|entry| {
                                let is_open = open.as_deref() == Some(entry.id.as_str());
                                entry_row(entry, is_open, expanded)
                            })
                            .collect_view()
                            .into_any()
                    }
                }}
            </section>
        }
    }

    /// The empty-list message, distinguishing an in-flight first load from a
    /// genuinely-empty browse and from a search that found nothing.
    fn empty_state(busy: bool, loaded: bool, searching: bool) -> AnyView {
        if busy && !loaded {
            view! { <p class="empty muted">"Loading\u{2026}"</p> }.into_any()
        } else if searching {
            view! { <p class="empty muted">"No matches. Try different words."</p> }.into_any()
        } else {
            view! { <p class="empty muted">{EMPTY_BROWSE}</p> }.into_any()
        }
    }

    /// One entry row: a tappable accordion header (snippet + tag chips + date)
    /// that reveals the full content when `is_open`. Tapping toggles `expanded`,
    /// which re-renders the list (so at most one row is open at a time).
    fn entry_row(
        entry: KnowledgeEntryView,
        is_open: bool,
        expanded: RwSignal<Option<String>>,
    ) -> AnyView {
        let id = entry.id.clone();
        let toggle = move |_| {
            expanded.update(|cur| {
                if cur.as_deref() == Some(id.as_str()) {
                    *cur = None;
                } else {
                    *cur = Some(id.clone());
                }
            });
        };

        let preview = snippet(&entry.content, SNIPPET_CHARS);
        let meta = meta_line(&entry.created_at, &entry.updated_at);
        let tags = entry.tags.clone();
        let full_content = entry.content.clone();

        let tag_chips = (!tags.is_empty()).then(|| {
            view! {
                <span class="kb-tags">
                    {tags
                        .into_iter()
                        .map(|t| view! { <span class="kb-tag">{t}</span> })
                        .collect_view()}
                </span>
            }
        });

        let detail = is_open.then(|| {
            view! {
                <div class="kb-entry-detail">
                    <p class="kb-full-content">{full_content}</p>
                </div>
            }
        });

        view! {
            <div class="kb-entry" class:expanded=is_open>
                <button
                    class="kb-entry-head"
                    aria-expanded=if is_open { "true" } else { "false" }
                    on:click=toggle
                >
                    <span class="kb-snippet">{preview}</span>
                    <span class="kb-entry-foot">
                        <span class="kb-meta muted">{meta}</span>
                        {tag_chips}
                    </span>
                </button>
                {detail}
            </div>
        }
        .into_any()
    }
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

    #[test]
    fn meta_line_added_only_when_never_updated() {
        // created == updated (a fresh, untouched entry) → no redundant "Updated".
        assert_eq!(
            meta_line("2026-07-14 00:00:00", "2026-07-14 00:00:00"),
            "Added 2026-07-14"
        );
        // updated missing → still just "Added".
        assert_eq!(meta_line("2026-07-14 00:00:00", ""), "Added 2026-07-14");
    }

    #[test]
    fn meta_line_shows_update_when_it_differs() {
        assert_eq!(
            meta_line("2026-07-14 00:00:00", "2026-07-20 09:30:00"),
            "Added 2026-07-14 \u{00b7} Updated 2026-07-20"
        );
    }

    #[test]
    fn meta_line_tolerates_missing_created() {
        assert_eq!(meta_line("", ""), "", "no dates → empty meta line");
        assert_eq!(meta_line("", "2026-07-20 09:30:00"), "Updated 2026-07-20");
    }

    #[test]
    fn constants_are_sane() {
        assert!(KB_LIMIT >= 1, "must read at least one entry");
        assert!(
            SNIPPET_CHARS >= 20,
            "a preview shorter than ~20 chars is useless"
        );
    }
}
