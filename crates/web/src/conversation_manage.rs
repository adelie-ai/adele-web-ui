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
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed == current {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Keep only the archived conversations from a list fetched with
/// `include_archived: true` (which returns active AND archived). The switcher's
/// archived section shows only these; the active list is the reducer-owned
/// default (`include_archived: false`) fetch.
pub fn archived_only(convs: Vec<ConversationSummary>) -> Vec<ConversationSummary> {
    convs.into_iter().filter(|c| c.archived).collect()
}

// ===========================================================================
// Leptos views (wasm only)
// ===========================================================================

#[cfg(target_arch = "wasm32")]
pub mod view {
    use std::rc::Rc;

    use desktop_assistant_api_model::client::ConversationSummary;
    use leptos::prelude::*;

    use super::effective_rename;
    use crate::settings::EngineHandle;
    use crate::sidebar::{display_title, message_count_label};

    /// A single conversation row for the active list, with the switcher's full
    /// action set. Three mutually-exclusive shapes replace the row in place so a
    /// destructive/edit action always takes a deliberate step:
    /// - `is_renaming` → an inline rename editor (input prefilled with the stored
    ///   title, submitted on Enter or Save; blank/unchanged is a no-op);
    /// - `is_confirming` → the delete confirm (unchanged from the base switcher);
    /// - otherwise → the tappable select area plus rename / archive / delete
    ///   icon buttons (archive needs no confirm — it is reversible via the
    ///   archived section below).
    #[allow(clippy::too_many_arguments)] // Row state is a flat set of small flags/signals; a struct would add indirection without clarifying.
    pub fn conversation_row(
        engine: EngineHandle,
        open: RwSignal<bool>,
        pending_delete: RwSignal<Option<String>>,
        renaming: RwSignal<Option<String>>,
        summary: ConversationSummary,
        active: bool,
        is_confirming: bool,
        is_renaming: bool,
    ) -> AnyView {
        let id = summary.id.clone();
        let title = display_title(&summary).to_string();
        let subtitle = message_count_label(summary.message_count);

        if is_renaming {
            // Prefill with the actual stored title (not the "Untitled" display
            // fallback), so naming a blank conversation starts from an empty
            // field rather than the placeholder word.
            let stored_title = summary.title.clone();
            let draft = RwSignal::new(summary.title.clone());
            let save_id = id.clone();
            let on_submit = move |ev: leptos::ev::SubmitEvent| {
                ev.prevent_default();
                if let Some(new_title) = effective_rename(&stored_title, &draft.get()) {
                    engine
                        .with_value(|e| e.borrow().rename_conversation(save_id.clone(), new_title));
                }
                renaming.set(None);
            };
            let cancel = move |_| renaming.set(None);
            return view! {
                <form class="conv-rename" role="listitem" on:submit=on_submit>
                    <input
                        class="conv-rename-input"
                        type="text"
                        placeholder="Conversation name"
                        autocomplete="off"
                        prop:value=move || draft.get()
                        on:input=move |ev| draft.set(event_target_value(&ev))
                    />
                    <div class="conv-rename-actions">
                        <button type="button" class="conv-btn" on:click=cancel>
                            "Cancel"
                        </button>
                        <button type="submit" class="conv-btn accent">
                            "Save"
                        </button>
                    </div>
                </form>
            }
            .into_any();
        }

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
        let rename_id = id.clone();
        let start_rename = move |ev: leptos::ev::MouseEvent| {
            ev.stop_propagation();
            pending_delete.set(None);
            renaming.set(Some(rename_id.clone()));
        };
        let archive_id = id.clone();
        let do_archive = move |ev: leptos::ev::MouseEvent| {
            ev.stop_propagation();
            engine.with_value(|e| e.borrow().archive_conversation(archive_id.clone()));
        };
        let ask_delete = move |ev: leptos::ev::MouseEvent| {
            ev.stop_propagation();
            renaming.set(None);
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
                    class="icon-btn conv-rename-btn"
                    aria-label="Rename conversation"
                    on:click=start_rename
                >
                    "\u{270E}"
                </button>
                <button
                    class="icon-btn conv-archive"
                    aria-label="Archive conversation"
                    on:click=do_archive
                >
                    "\u{1F5C4}"
                </button>
                <button
                    class="icon-btn conv-del"
                    aria-label="Delete conversation"
                    on:click=ask_delete
                >
                    "\u{1F5D1}"
                </button>
            </div>
        }
        .into_any()
    }

    /// The collapsed-by-default "Archived" disclosure at the foot of the drawer.
    /// Expanding it fetches the archived conversations on demand (an
    /// `include_archived` list, filtered engine-side) into a local signal — the
    /// reducer never sees them, so archived rows can't leak into the default
    /// list — and renders each with an Unarchive action. Unarchiving restores the
    /// conversation to the default list and drops it from here.
    pub fn archived_section(engine: EngineHandle) -> impl IntoView {
        let expanded = RwSignal::new(false);
        let loading = RwSignal::new(false);
        let archived = RwSignal::new(Vec::<ConversationSummary>::new());

        let toggle = move |_| {
            let now = !expanded.get();
            expanded.set(now);
            if now {
                loading.set(true);
                let sink = archived;
                let done = loading;
                engine.with_value(move |e| {
                    e.borrow().fetch_archived_conversations(Rc::new(
                        move |list: Vec<ConversationSummary>| {
                            sink.set(list);
                            done.set(false);
                        },
                    ))
                });
            }
        };

        view! {
            <div class="conv-archived">
                <button
                    class="conv-archived-toggle"
                    aria-label="Show archived conversations"
                    aria-expanded=move || if expanded.get() { "true" } else { "false" }
                    on:click=toggle
                >
                    {move || {
                        if expanded.get() {
                            "\u{25BE} Archived"
                        } else {
                            "\u{25B8} Archived"
                        }
                    }}
                </button>
                <Show when=move || expanded.get()>
                    <div class="conv-archived-list" role="list">
                        {move || archived_rows(engine, archived, loading)}
                    </div>
                </Show>
            </div>
        }
    }

    /// The archived section's body: a loading line, an empty state, or one
    /// [`archived_row`] per archived conversation.
    fn archived_rows(
        engine: EngineHandle,
        archived: RwSignal<Vec<ConversationSummary>>,
        loading: RwSignal<bool>,
    ) -> AnyView {
        if loading.get() {
            return view! { <p class="empty muted">"Loading\u{2026}"</p> }.into_any();
        }
        let rows = archived.get();
        if rows.is_empty() {
            return view! { <p class="empty muted">"No archived conversations."</p> }.into_any();
        }
        rows.into_iter()
            .map(|summary| archived_row(engine, archived, summary))
            .collect_view()
            .into_any()
    }

    /// One archived conversation: its title/subtitle (informational — archived
    /// rows are not tappable-to-open) and an Unarchive button. Unarchiving calls
    /// the engine, which restores it to the default list and refreshes this
    /// section (`archived`) so the row leaves it.
    fn archived_row(
        engine: EngineHandle,
        archived: RwSignal<Vec<ConversationSummary>>,
        summary: ConversationSummary,
    ) -> impl IntoView {
        let id = summary.id.clone();
        let title = display_title(&summary).to_string();
        let subtitle = message_count_label(summary.message_count);
        let unarchive = move |_| {
            let sink = archived;
            let uid = id.clone();
            engine.with_value(move |e| {
                e.borrow().unarchive_conversation(
                    uid,
                    Rc::new(move |list: Vec<ConversationSummary>| sink.set(list)),
                )
            });
        };
        view! {
            <div class="conv-item archived" role="listitem">
                <div class="conv-row conv-row-static">
                    <span class="conv-title">{title}</span>
                    <span class="conv-sub muted">{subtitle}</span>
                </div>
                <button
                    class="conv-btn conv-unarchive"
                    aria-label="Unarchive conversation"
                    on:click=unarchive
                >
                    "Unarchive"
                </button>
            </div>
        }
    }
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
