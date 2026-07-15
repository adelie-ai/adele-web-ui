//! Conversation **rename** and **archive/unarchive** for the switcher (issue
//! #49): the pure, host-testable decision logic, plus (on wasm) the per-row
//! action buttons, the inline rename editor, and the archived section layered
//! over the existing sidebar (`crate::sidebar`).
//!
//! State lives in the shared reducer where it already models these mutations,
//! not in a parallel machine here:
//! - **rename** flows through `UiMessage::ConversationRenamed` (which patches the
//!   sidebar list row) and, for the *open* conversation, a re-fetch delivered as
//!   `ConversationReloaded` so the header title — derived from the open detail,
//!   not the summary list — updates too;
//! - **archive** reuses `UiMessage::ConversationDeleted` to drop the row from the
//!   default (non-archived) list and re-home the view — the conversation is not
//!   deleted server-side, so `include_archived` still lists it and unarchive
//!   restores it.
//!
//! The **archived list itself** is a view-only concern (a separate
//! `include_archived` fetch that the reducer deliberately does not model, so
//! archived rows never leak into the default list). It lives in a local signal
//! owned by the archived section, fed by the engine via a one-shot callback.

use desktop_assistant_api_model::client::ConversationSummary;

// ===========================================================================
// Pure logic (host-testable)
// ===========================================================================

/// Trim a user-entered title and decide whether renaming `current` to it is a
/// real change worth sending to the daemon.
///
/// Returns `Some(trimmed)` — the effective new title — only when the trimmed
/// input is non-empty AND differs from the current stored title. Returns `None`
/// when the input is blank (whitespace-only) — a blank rename is rejected,
/// keeping the existing title — or when it equals the current title (a no-op, so
/// no command is issued). Leading/trailing whitespace is always trimmed, so
/// `" Trip "` and `"Trip"` are the same rename (and a no-op against a stored
/// `"Trip"`).
pub fn effective_rename(current: &str, input: &str) -> Option<String> {
    // Spec stub — real body lands in the implementation commit.
    let _ = (current, input);
    None
}

/// Keep only the archived conversations from a list fetched with
/// `include_archived: true` (which returns active AND archived). The switcher's
/// archived section shows only these; the active list is the reducer-owned
/// default (`include_archived: false`) fetch.
pub fn archived_only(convs: Vec<ConversationSummary>) -> Vec<ConversationSummary> {
    // Spec stub — real body lands in the implementation commit.
    let _ = convs;
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(id: &str, title: &str, archived: bool) -> ConversationSummary {
        ConversationSummary {
            id: id.to_string(),
            title: title.to_string(),
            message_count: 0,
            archived,
        }
    }

    #[test]
    fn effective_rename_returns_trimmed_change() {
        // A genuine change yields the trimmed new title to send.
        assert_eq!(
            effective_rename("Trip planning", "Trip planning 2026"),
            Some("Trip planning 2026".to_string())
        );
        // Surrounding whitespace is stripped from the sent title.
        assert_eq!(
            effective_rename("Trip", "  Holiday  "),
            Some("Holiday".to_string())
        );
    }

    #[test]
    fn effective_rename_rejects_blank() {
        // Empty and whitespace-only inputs are rejected (keep the old title).
        assert_eq!(effective_rename("Trip", ""), None);
        assert_eq!(effective_rename("Trip", "   "), None);
        assert_eq!(effective_rename("Trip", "\t\n"), None);
    }

    #[test]
    fn effective_rename_is_noop_when_unchanged() {
        // Identical, or differing only by surrounding whitespace, is a no-op —
        // no command should be sent.
        assert_eq!(effective_rename("Trip", "Trip"), None);
        assert_eq!(effective_rename("Trip", "  Trip  "), None);
    }

    #[test]
    fn effective_rename_allows_naming_a_blank_titled_conversation() {
        // A conversation whose stored title is empty (a just-created chat before
        // its first auto-title) can be named for the first time.
        assert_eq!(
            effective_rename("", "First name"),
            Some("First name".to_string())
        );
        // ...but blank-to-blank is still a no-op.
        assert_eq!(effective_rename("", "   "), None);
    }

    #[test]
    fn archived_only_keeps_archived_and_drops_active() {
        let convs = vec![
            summary("c1", "Active one", false),
            summary("c2", "Archived one", true),
            summary("c3", "Active two", false),
            summary("c4", "Archived two", true),
        ];
        let archived = archived_only(convs);
        let ids: Vec<&str> = archived.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["c2", "c4"]);
        assert!(archived.iter().all(|c| c.archived));
    }

    #[test]
    fn archived_only_empty_when_none_archived() {
        let convs = vec![
            summary("c1", "Active one", false),
            summary("c2", "Active two", false),
        ];
        assert!(archived_only(convs).is_empty());
    }

    #[test]
    fn archived_only_empty_input() {
        assert!(archived_only(Vec::new()).is_empty());
    }
}
