//! Message-queuing composer view logic (feat/queue-messages).
//!
//! The queue *state* — enqueue-while-streaming, edit/remove, and the combined
//! flush on stream-complete — lives entirely in the shared `client-ui-common`
//! reducer (`WindowState`), so the SPA can never disagree with the other
//! clients. This module holds only the two web-specific presentation concerns
//! the shared model doesn't own, kept transport-/view-free so they unit-test on
//! the host target like [`crate::model`] / [`crate::context`]:
//!
//! 1. [`chip_preview`] — a compact, single-line label for a queued-message chip.
//! 2. [`recall_up`] / [`recall_down`] — the up/down-arrow recall walk over the
//!    queue, translated into the reducer message the keydown handler dispatches.
//!
//! The `#[cfg(target_arch = "wasm32")]` Leptos strip (the chips row above the
//! composer) lives at the bottom.

/// Max characters shown on a queued-message chip before it is elided.
pub const CHIP_PREVIEW_MAX: usize = 40;

/// A compact, single-line preview of a queued message for a chip label.
///
/// Queued messages can be multi-line (the combined flush joins them with `\n`),
/// so any internal whitespace is collapsed to single spaces to keep a chip one
/// tidy line, then the text is elided to `max` characters (an ellipsis takes the
/// last slot). Truncation is on `char` boundaries so multi-byte UTF-8 never
/// panics. An empty/whitespace-only preview stays empty (the reducer never
/// queues blank text, but the helper is total).
pub fn chip_preview(text: &str, max: usize) -> String {
    // STUB (red): real impl collapses whitespace + elides to `max` chars.
    let _ = (text, max);
    String::new()
}

/// What an ArrowUp/ArrowDown recall keystroke should ask the reducer to do.
///
/// The keydown handler maps this onto a reducer message: [`Edit`](Self::Edit) →
/// `UiMessage::EditQueued`, [`Cancel`](Self::Cancel) →
/// `UiMessage::CancelQueuedEdit`, [`None`](Self::None) → leave the keystroke to
/// the browser (no queue navigation applies).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecallAction {
    /// Check out the queued item at this outbox index for editing.
    Edit(usize),
    /// Abandon the in-progress edit, returning the item to the queue unchanged.
    Cancel,
    /// No queue navigation applies.
    None,
}

/// The ArrowUp (recall-backward) walk.
///
/// `editing` is the reducer's `editing_queued_index()` (the full-queue index of
/// the item currently checked out into the composer, or `None`); `view_queue_len`
/// is `queued_messages_for_view().len()` — which *excludes* any checked-out item.
///
/// - Not editing: check out the *last* queued item (`Edit(len - 1)`), or `None`
///   when the queue is empty.
/// - Editing item `i`: step to the previous item (`Edit(i - 1)`), or `None` at
///   the first item (`i == 0`), where there is nowhere earlier to go.
pub fn recall_up(editing: Option<usize>, view_queue_len: usize) -> RecallAction {
    // STUB (red): real impl walks backward from the last queued item.
    let _ = (editing, view_queue_len);
    RecallAction::None
}

/// The ArrowDown (recall-forward) walk, meaningful only while editing.
///
/// `editing` / `view_queue_len` are as in [`recall_up`]. The full queue length
/// (counting the checked-out item) is `view_queue_len + 1` while editing.
///
/// - Not editing: [`RecallAction::None`] — ArrowDown does nothing.
/// - Editing item `i`: step to the next item (`Edit(i + 1)`) when one exists,
///   else [`Cancel`](RecallAction::Cancel) — stepping past the last item returns
///   the checked-out message to the queue and drops back to a fresh composer.
pub fn recall_down(editing: Option<usize>, view_queue_len: usize) -> RecallAction {
    // STUB (red): real impl steps forward / cancels off the end.
    let _ = (editing, view_queue_len);
    RecallAction::None
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- chip_preview --------------------------------------------------------

    #[test]
    fn chip_preview_keeps_short_text_verbatim() {
        assert_eq!(chip_preview("hello there", 40), "hello there");
    }

    #[test]
    fn chip_preview_max_is_a_sensible_default() {
        // The chip budget the wasm strip passes; guarded so a careless edit that
        // set it to 0 (every chip collapses to just "…") is caught.
        assert!(CHIP_PREVIEW_MAX >= 8);
        assert_eq!(chip_preview("hi", CHIP_PREVIEW_MAX), "hi");
    }

    #[test]
    fn chip_preview_collapses_internal_whitespace() {
        // Newlines (from a multi-line queued message) and runs of spaces become
        // single spaces so the chip stays one line.
        assert_eq!(chip_preview("first\nsecond   third", 40), "first second third");
    }

    #[test]
    fn chip_preview_elides_overlong_text_with_ellipsis() {
        let text = "abcdefghijklmnopqrstuvwxyz"; // 26 chars
        // max 10 -> 9 chars + ellipsis.
        assert_eq!(chip_preview(text, 10), "abcdefghi…");
    }

    #[test]
    fn chip_preview_at_exact_max_is_not_elided() {
        assert_eq!(chip_preview("abcde", 5), "abcde");
    }

    #[test]
    fn chip_preview_truncates_on_char_boundaries() {
        // Multi-byte chars must not be split mid-codepoint (no panic, no mojibake).
        let text = "áéíóúñ mangoes"; // leading accented chars are 2 bytes each
        let out = chip_preview(text, 4);
        assert_eq!(out.chars().count(), 4); // 3 chars + ellipsis
        assert!(out.ends_with('…'));
        assert!(out.starts_with("áéí"));
    }

    #[test]
    fn chip_preview_empty_stays_empty() {
        assert_eq!(chip_preview("   \n  ", 40), "");
    }

    // --- recall_up (ArrowUp, walk backward) ----------------------------------

    #[test]
    fn recall_up_from_idle_checks_out_last_queued() {
        // Queue ["a","b","c"], not editing -> check out index 2 ("c").
        assert_eq!(recall_up(None, 3), RecallAction::Edit(2));
    }

    #[test]
    fn recall_up_from_idle_empty_queue_is_none() {
        assert_eq!(recall_up(None, 0), RecallAction::None);
    }

    #[test]
    fn recall_up_while_editing_steps_to_previous() {
        // Editing full-queue index 2, one item checked out so view len is 2.
        assert_eq!(recall_up(Some(2), 2), RecallAction::Edit(1));
        assert_eq!(recall_up(Some(1), 2), RecallAction::Edit(0));
    }

    #[test]
    fn recall_up_at_first_item_is_none() {
        // Already editing the earliest item: nowhere earlier to walk.
        assert_eq!(recall_up(Some(0), 2), RecallAction::None);
    }

    // --- recall_down (ArrowDown, walk forward) -------------------------------

    #[test]
    fn recall_down_when_not_editing_is_none() {
        assert_eq!(recall_down(None, 3), RecallAction::None);
    }

    #[test]
    fn recall_down_steps_to_next_item() {
        // Full queue length 3 (view len 2 while editing). From index 0 -> 1, 1 -> 2.
        assert_eq!(recall_down(Some(0), 2), RecallAction::Edit(1));
        assert_eq!(recall_down(Some(1), 2), RecallAction::Edit(2));
    }

    #[test]
    fn recall_down_off_the_last_item_cancels() {
        // Editing the last item (index 2 of a length-3 queue): ArrowDown returns
        // it to the queue and drops back to a fresh composer.
        assert_eq!(recall_down(Some(2), 2), RecallAction::Cancel);
    }

    #[test]
    fn recall_down_single_item_queue_cancels() {
        // One queued item checked out (view len 0, full len 1): down cancels.
        assert_eq!(recall_down(Some(0), 0), RecallAction::Cancel);
    }
}
