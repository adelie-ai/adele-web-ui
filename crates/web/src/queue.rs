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
    // Collapse any run of whitespace (including the `\n` joins of a multi-line
    // queued message) to a single space so the chip stays one tidy line.
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max {
        return collapsed;
    }
    // Reserve the final slot for the ellipsis; truncate on a `char` boundary.
    let head: String = collapsed.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
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
    match editing {
        // Not editing: the whole queue is visible, so its last index is
        // `len - 1`. An empty queue has nothing to recall.
        None => match view_queue_len {
            0 => RecallAction::None,
            len => RecallAction::Edit(len - 1),
        },
        // Already at the earliest item: nowhere earlier to walk.
        Some(0) => RecallAction::None,
        // `i` is the full-queue index; the reducer reinserts the currently
        // checked-out item before checking out `i - 1`, so the index is absolute.
        Some(i) => RecallAction::Edit(i - 1),
    }
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
    match editing {
        None => RecallAction::None,
        Some(i) => {
            // The checked-out item is absent from the view, so the full queue is
            // one longer. Step forward while a next item exists; off the end,
            // cancel back to a fresh composer.
            let full_len = view_queue_len + 1;
            if i + 1 < full_len {
                RecallAction::Edit(i + 1)
            } else {
                RecallAction::Cancel
            }
        }
    }
}

/// Translate a queued-chip's position in the *view* list into the full-queue
/// index to pass to `UiMessage::EditQueued`.
///
/// While an item is checked out for editing (`editing == Some(e)`) it is absent
/// from the view list (`queued_messages_for_view`), so every chip at or after its
/// slot is shifted down by one. `EditQueued` reinserts the checked-out item
/// before checking out the new one, so it expects a *full*-queue index: a chip at
/// view position `>= e` is really full-queue position `+ 1`. With nothing checked
/// out, the view list IS the queue and the index passes through unchanged.
///
/// Only [`RecallAction::Edit`] from a chip *tap* needs this — the up/down recall
/// walk already works in full-queue indices, and `RemoveQueued` deletes straight
/// from the view list, so neither is translated.
pub fn chip_edit_index(view_index: usize, editing: Option<usize>) -> usize {
    match editing {
        Some(e) if view_index >= e => view_index + 1,
        _ => view_index,
    }
}

/// Whether the composer should be cleared after a submit fires (AC9).
///
/// A submit into an *idle* conversation sends immediately, so the just-sent text
/// is cleared here. A submit while a reply *streams* is QUEUED by the reducer,
/// which clears the composer itself via `Effect::SetComposerText`; the handler
/// must NOT also clear, or it would blank a fresh draft the user starts typing
/// while the reply streams. So: clear iff not streaming. Extracted from the
/// composer's submit handler (an un-unit-testable Leptos closure) so the
/// decision is pinned by a host test.
pub fn should_clear_composer_on_submit(streaming: bool) -> bool {
    !streaming
}

/// Whether an ArrowUp keystroke should start/step a queue recall (AC9).
///
/// - Editing (`editing.is_some()`): always walk — the composer holds the
///   checked-out item, not a fresh draft, so ArrowUp steps to the previous item.
/// - Not editing: recall ONLY from an *empty* composer that has a non-empty
///   queue, so ArrowUp never clobbers a draft the user is part-way through
///   typing. Both terms matter: a non-empty draft must block recall even with a
///   queue present, and an empty composer with no queue has nothing to recall.
///
/// Extracted from the composer's keydown closure (wasm-gated, not unit-testable)
/// so the guard — in particular the `composer_empty` term whose loss would let
/// ArrowUp overwrite a draft — is pinned by a host test.
pub fn should_recall_on_arrow_up(
    editing: Option<usize>,
    composer_empty: bool,
    queue_len: usize,
) -> bool {
    editing.is_some() || (composer_empty && queue_len > 0)
}

#[cfg(target_arch = "wasm32")]
pub use view::queued_chips;

#[cfg(target_arch = "wasm32")]
mod view {
    use leptos::prelude::*;

    use super::{CHIP_PREVIEW_MAX, chip_preview};
    use crate::engine::ViewSignals;
    use crate::settings::EngineHandle;

    /// The queued-messages strip, shown just above the composer whenever the open
    /// conversation has messages queued (submitted while Adele was busy and not
    /// yet flushed). A leading "N queued" count frames the batch; each chip
    /// previews one queued message. Tapping a chip's body checks it out into the
    /// composer to edit (`EditQueued`); its × drops it (`RemoveQueued`). Hidden
    /// (zero footprint) when the queue is empty, so it never crowds the phone
    /// chat. The strip scrolls horizontally rather than wrapping, so the composer
    /// keeps a stable height as the batch grows.
    ///
    /// The queued texts + the checked-out index come from the shared reducer
    /// (mirrored into `view.queued` / `view.editing_queued`); the count folds the
    /// checked-out item back in so it matches the user's mental batch size while
    /// an item is being edited.
    pub fn queued_chips(engine: EngineHandle, view: ViewSignals) -> impl IntoView {
        move || {
            let queued = view.queued.get();
            let editing = view.editing_queued.get();
            // The count folds the checked-out item (absent from `queued`) back in
            // so "N queued" matches the user's mental batch size while editing.
            let total = queued.len() + usize::from(editing.is_some());
            if total == 0 {
                return None;
            }
            let chips = queued
                .into_iter()
                .enumerate()
                .map(|(index, text)| {
                    let label = chip_preview(&text, CHIP_PREVIEW_MAX);
                    // Editing tap needs the full-queue index (the checked-out item
                    // is missing from this list); remove deletes from the list as-is.
                    let edit_index = super::chip_edit_index(index, editing);
                    let edit =
                        move |_| engine.with_value(|e| e.borrow_mut().edit_queued(edit_index));
                    let remove =
                        move |_| engine.with_value(|e| e.borrow_mut().remove_queued(index));
                    view! {
                        <span class="queued-chip" role="listitem">
                            <button
                                class="queued-chip-edit"
                                title=text
                                on:click=edit
                            >
                                {label}
                            </button>
                            <button
                                class="queued-chip-remove"
                                aria-label="Remove queued message"
                                on:click=remove
                            >
                                "\u{2715}"
                            </button>
                        </span>
                    }
                })
                .collect_view();
            Some(view! {
                <div class="queued-strip" role="list" aria-label="Queued messages">
                    <span class="queued-count">{format!("{total} queued")}</span>
                    {chips}
                </div>
            })
        }
    }
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
        assert_eq!(
            chip_preview("first\nsecond   third", 40),
            "first second third"
        );
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

    // --- chip_edit_index (chip-tap view-index -> full-queue index) -----------

    #[test]
    fn chip_edit_index_passthrough_when_not_editing() {
        // Nothing checked out: the view list IS the queue, indices pass through.
        assert_eq!(chip_edit_index(0, None), 0);
        assert_eq!(chip_edit_index(3, None), 3);
    }

    #[test]
    fn chip_edit_index_shifts_chips_at_or_after_the_hole() {
        // Full ["a","b","c","d"], "c" (full-index 2) checked out -> view
        // ["a"(0),"b"(1),"d"(2)]. Chips before the hole are unchanged; the chip at
        // the hole and after map to their full-queue positions (+1).
        assert_eq!(chip_edit_index(0, Some(2)), 0); // a
        assert_eq!(chip_edit_index(1, Some(2)), 1); // b
        assert_eq!(chip_edit_index(2, Some(2)), 3); // d, not c
    }

    #[test]
    fn chip_edit_index_hole_at_front_shifts_all() {
        // "a" (full-index 0) checked out -> every remaining chip is shifted by one.
        assert_eq!(chip_edit_index(0, Some(0)), 1);
        assert_eq!(chip_edit_index(1, Some(0)), 2);
    }

    #[test]
    fn chip_edit_index_editing_last_leaves_earlier_chips_unshifted() {
        // Full ["a","b","c"], the last item "c" (full-index 2) checked out -> view
        // ["a"(0),"b"(1)]. Every visible chip sits before the hole, so none shift;
        // no visible index ever reaches the editing slot (the +1 branch is unused).
        assert_eq!(chip_edit_index(0, Some(2)), 0);
        assert_eq!(chip_edit_index(1, Some(2)), 1);
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

    // --- should_clear_composer_on_submit (AC9) -------------------------------

    #[test]
    fn submit_while_idle_clears_the_composer() {
        // Idle send: the just-sent text is cleared by the handler.
        assert!(should_clear_composer_on_submit(false));
    }

    #[test]
    fn submit_while_streaming_preserves_the_composer_draft() {
        // While a reply streams the send is queued (the reducer clears the
        // composer via SetComposerText); the handler must NOT also clear, or it
        // would blank a fresh draft typed during the stream. Dropping this term
        // (clearing unconditionally) is the AC9 regression this pins.
        assert!(!should_clear_composer_on_submit(true));
    }

    // --- should_recall_on_arrow_up (AC9) -------------------------------------

    #[test]
    fn arrowup_does_not_recall_over_nonempty_draft() {
        // A non-empty draft with a queue present must NOT recall — losing the
        // `composer_empty` term would overwrite the draft. This is the exact AC9
        // regression the guard defends against.
        assert!(!should_recall_on_arrow_up(None, false, 2));
    }

    #[test]
    fn arrowup_recalls_from_empty_composer_with_a_queue() {
        // Empty composer + a queued message: ArrowUp starts the recall.
        assert!(should_recall_on_arrow_up(None, true, 2));
    }

    #[test]
    fn arrowup_does_not_recall_from_empty_composer_with_empty_queue() {
        // Nothing queued: there is nothing to recall even from an empty composer.
        assert!(!should_recall_on_arrow_up(None, true, 0));
    }

    #[test]
    fn arrowup_while_editing_always_walks_regardless_of_draft() {
        // Editing: the composer holds the checked-out item, so ArrowUp walks even
        // though the composer is "non-empty".
        assert!(should_recall_on_arrow_up(Some(1), false, 2));
    }
}
