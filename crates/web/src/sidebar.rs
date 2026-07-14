//! The conversation switcher (issue #12): a mobile-first slide-in drawer listing
//! the user's conversations, with the active one marked, a "new conversation"
//! affordance, and per-row delete.
//!
//! Split like [`crate::model`] / [`crate::connections`]: the pure row-render
//! helpers (title fallback, message-count subtitle, active-row test) are
//! transport-/view-free so they compile and unit-test on the host target. The
//! Leptos drawer (`#[cfg(target_arch = "wasm32")]`) and the engine commands
//! (`select_conversation` / `new_conversation` / `delete_conversation`) are the
//! thin wasm shell over that logic.
//!
//! State lives in the shared reducer, not here: the list is the reducer's
//! `conversations` (mirrored into [`ViewSignals::conversations`] via the
//! `SetConversations` effect) and the marked row is its
//! `current_conversation_id`. Switching is a `GetConversation` fetch fed back as
//! `ConversationLoaded`; deleting flows through `ConversationDeleted`, which the
//! reducer uses to drop the row and re-home the view — so this module never
//! grows a parallel conversation state machine.

use desktop_assistant_api_model::client::ConversationSummary;

// ===========================================================================
// Pure logic (host-testable)
// ===========================================================================

/// The label to render for a conversation row: the stored title, or an
/// "Untitled" fallback when it is empty/whitespace-only. A blank title (a
/// just-created conversation before its first auto-title, or a daemon that
/// stored an empty string) would otherwise paint an unlabeled — yet still
/// tappable — row, so it always resolves to something legible.
pub fn display_title(summary: &ConversationSummary) -> &str {
    let _ = summary;
    "" // TODO(#12): stub — real impl returns the title or an "Untitled" fallback.
}

/// A one-line subtitle describing how many messages a conversation holds,
/// pluralized: `0 -> "No messages yet"`, `1 -> "1 message"`, `n -> "n messages"`.
/// Gives each row a mobile-friendly second line without the caller re-deriving
/// the wording.
pub fn message_count_label(count: u32) -> String {
    let _ = count;
    String::new() // TODO(#12): stub — real impl pluralizes.
}

/// Whether `summary` is the conversation currently open in the chat view — the
/// switcher marks that row. `false` when nothing is open (`current` is `None`)
/// or a different conversation is open.
pub fn is_active(summary: &ConversationSummary, current: Option<&str>) -> bool {
    let _ = (summary, current);
    false // TODO(#12): stub — real impl compares ids.
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(id: &str, title: &str, message_count: u32) -> ConversationSummary {
        ConversationSummary {
            id: id.to_string(),
            title: title.to_string(),
            message_count,
            archived: false,
        }
    }

    #[test]
    fn display_title_uses_stored_title() {
        assert_eq!(display_title(&summary("c1", "Trip planning", 3)), "Trip planning");
    }

    #[test]
    fn display_title_falls_back_when_blank() {
        // An empty title and a whitespace-only title both resolve to "Untitled"
        // rather than painting an unlabeled row.
        assert_eq!(display_title(&summary("c1", "", 0)), "Untitled");
        assert_eq!(display_title(&summary("c2", "   ", 0)), "Untitled");
    }

    #[test]
    fn message_count_label_pluralizes() {
        assert_eq!(message_count_label(0), "No messages yet");
        assert_eq!(message_count_label(1), "1 message");
        assert_eq!(message_count_label(2), "2 messages");
        assert_eq!(message_count_label(42), "42 messages");
    }

    #[test]
    fn is_active_matches_current() {
        let s = summary("c1", "One", 1);
        assert!(is_active(&s, Some("c1")));
    }

    #[test]
    fn is_active_false_when_none_open() {
        let s = summary("c1", "One", 1);
        assert!(!is_active(&s, None));
    }

    #[test]
    fn is_active_false_for_a_different_conversation() {
        let s = summary("c1", "One", 1);
        assert!(!is_active(&s, Some("c2")));
    }
}
