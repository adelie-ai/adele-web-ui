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
    if summary.title.trim().is_empty() {
        "Untitled"
    } else {
        summary.title.as_str()
    }
}

/// A one-line subtitle describing how many messages a conversation holds,
/// pluralized: `0 -> "No messages yet"`, `1 -> "1 message"`, `n -> "n messages"`.
/// Gives each row a mobile-friendly second line without the caller re-deriving
/// the wording.
pub fn message_count_label(count: u32) -> String {
    match count {
        0 => "No messages yet".to_string(),
        1 => "1 message".to_string(),
        n => format!("{n} messages"),
    }
}

/// Whether `summary` is the conversation currently open in the chat view — the
/// switcher marks that row. `false` when nothing is open (`current` is `None`)
/// or a different conversation is open.
pub fn is_active(summary: &ConversationSummary, current: Option<&str>) -> bool {
    current == Some(summary.id.as_str())
}

// ===========================================================================
// Leptos view (wasm only)
// ===========================================================================

#[cfg(target_arch = "wasm32")]
pub use view::conversation_sidebar;

#[cfg(target_arch = "wasm32")]
mod view {
    use desktop_assistant_api_model::client::ConversationSummary;
    use leptos::prelude::*;

    use super::{display_title, is_active, message_count_label};
    use crate::engine::ViewSignals;
    use crate::settings::EngineHandle;

    /// The conversation switcher drawer. Rendered whenever `open` is `true`: a
    /// left slide-in over a dim backdrop, sized for one-handed phone use (a full
    /// panel with a wide "new" button and generous rows); on a wide viewport the
    /// same drawer narrows to a left rail. Tapping the backdrop or the close
    /// button dismisses it.
    pub fn conversation_sidebar(
        engine: EngineHandle,
        view: ViewSignals,
        open: RwSignal<bool>,
    ) -> impl IntoView {
        // Which row (if any) is awaiting a delete confirm, keyed by id so a
        // repaint (list refresh / active-row change) can never confirm a row the
        // user didn't point at.
        let pending_delete = RwSignal::new(None::<String>);

        let close = move |_| {
            pending_delete.set(None);
            open.set(false);
        };
        let start_new = move |_| {
            engine.with_value(|e| e.borrow().new_conversation());
            pending_delete.set(None);
            open.set(false);
        };

        view! {
            <Show when=move || open.get()>
                // The dim backdrop dismisses on tap; taps inside the drawer don't
                // bubble to it (`stop_propagation`).
                <div class="sidebar-backdrop" on:click=close>
                    <div
                        class="sidebar-drawer"
                        role="dialog"
                        aria-modal="true"
                        aria-label="Conversations"
                        on:click=|ev| ev.stop_propagation()
                    >
                        <header class="sidebar-header">
                            <h2>"Conversations"</h2>
                            <button
                                class="icon-btn"
                                aria-label="Close conversations"
                                on:click=close
                            >
                                "\u{2715}"
                            </button>
                        </header>

                        <button class="conv-new" on:click=start_new>
                            "+ New conversation"
                        </button>

                        <div class="conv-list" role="list">
                            {move || conversation_rows(engine, view, open, pending_delete)}
                        </div>
                    </div>
                </div>
            </Show>
        }
    }

    /// The list body: one row per conversation, or an empty state. Re-runs when
    /// the list, the open conversation, or the pending-delete row changes.
    fn conversation_rows(
        engine: EngineHandle,
        view: ViewSignals,
        open: RwSignal<bool>,
        pending_delete: RwSignal<Option<String>>,
    ) -> AnyView {
        let convs = view.conversations.get();
        if convs.is_empty() {
            return view! {
                <p class="empty muted">"No conversations yet. Start a new one above."</p>
            }
            .into_any();
        }
        let current = view.current_conversation_id.get();
        let confirming = pending_delete.get();
        convs
            .into_iter()
            .map(|summary| {
                let active = is_active(&summary, current.as_deref());
                let is_confirming = confirming.as_deref() == Some(summary.id.as_str());
                conversation_row(engine, open, pending_delete, summary, active, is_confirming)
            })
            .collect_view()
            .into_any()
    }

    /// A single conversation row — or, when this row is pending a delete, an
    /// inline Cancel/Delete confirm in its place (so a destructive tap always
    /// takes a deliberate second tap).
    fn conversation_row(
        engine: EngineHandle,
        open: RwSignal<bool>,
        pending_delete: RwSignal<Option<String>>,
        summary: ConversationSummary,
        active: bool,
        is_confirming: bool,
    ) -> AnyView {
        let id = summary.id.clone();
        let title = display_title(&summary).to_string();
        let subtitle = message_count_label(summary.message_count);

        if is_confirming {
            let confirm_id = id.clone();
            let do_delete = move |_| {
                engine.with_value(|e| e.borrow().delete_conversation(confirm_id.clone()));
                pending_delete.set(None);
            };
            let cancel = move |_| pending_delete.set(None);
            return view! {
                <div class="conv-confirm" role="listitem">
                    <span class="conv-confirm-q">
                        {format!("Delete \u{201c}{title}\u{201d}?")}
                    </span>
                    <div class="conv-confirm-actions">
                        <button class="conv-btn" on:click=cancel>"Cancel"</button>
                        <button class="conv-btn danger" on:click=do_delete>"Delete"</button>
                    </div>
                </div>
            }
            .into_any();
        }

        let select_id = id.clone();
        let on_select = move |_| {
            engine.with_value(|e| e.borrow().select_conversation(select_id.clone()));
            open.set(false);
        };
        let ask_delete = move |ev: leptos::ev::MouseEvent| {
            // Keep the tap off the row's select handler.
            ev.stop_propagation();
            pending_delete.set(Some(id.clone()));
        };

        view! {
            <div class="conv-item" class:active=active role="listitem">
                <button
                    class="conv-row"
                    aria-current=if active { "true" } else { "false" }
                    on:click=on_select
                >
                    <span class="conv-title">{title}</span>
                    <span class="conv-sub muted">{subtitle}</span>
                </button>
                <button
                    class="icon-btn conv-del"
                    aria-label="Delete conversation"
                    on:click=ask_delete
                >
                    "\u{1f5d1}"
                </button>
            </div>
        }
        .into_any()
    }
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
        assert_eq!(
            display_title(&summary("c1", "Trip planning", 3)),
            "Trip planning"
        );
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
