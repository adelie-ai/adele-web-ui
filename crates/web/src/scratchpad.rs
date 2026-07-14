//! Conversation scratchpad view (issue #16): a read-only look at the active
//! conversation's ephemeral working notes — the same per-conversation pad the
//! LLM manages with its builtin tools (DA#184/#240) and exposes to clients via
//! [`Command::GetConversationScratchpad`] → [`CommandResult::Scratchpad`].
//!
//! **Reducer-driven, not a parallel fetch loop.** The shared `client-ui-common`
//! reducer already models the pad end-to-end: on a conversation load/switch and
//! after every completed turn it emits `Effect::FetchScratchpad(id)`, and it
//! folds the fetched notes back out as `Effect::SidePaneSetScratchpad(notes)`
//! (guarded to the *active* conversation, so a fetch racing a switch is
//! dropped). The engine runs those two effects (see `engine.rs`) into a
//! `scratchpad` signal; this module only renders it. That means the pane stays
//! fresh as you chat — Adele's todos/notes appear turn-by-turn — with no
//! polling. (True live push while another client mutates the pad rides
//! `Event::ScratchpadChanged`; the reducer maps it to a refetch, and the wire
//! arm is in place, pending the BFF forwarding that background event — see the
//! PR's deferred-scope note. The turn-boundary refetch is the working live path
//! today.)
//!
//! **Split like `model.rs`/`personality.rs`.** The pure grouping/labelling/
//! summary logic lives at module top and unit-tests on the host target; the
//! Leptos panel is a `#[cfg(target_arch = "wasm32")]` submodule that consumes
//! *these* helpers, so the tested logic and the rendered logic can't drift.
//!
//! [`Command::GetConversationScratchpad`]: desktop_assistant_api_model::Command::GetConversationScratchpad
//! [`CommandResult::Scratchpad`]: desktop_assistant_api_model::CommandResult::Scratchpad

use desktop_assistant_api_model::ScratchpadNoteView;

/// True when a note is a to-do — its free-text `note_type` is `"todo"`,
/// compared case-insensitively so a stray `"TODO"` still renders with a
/// checkbox. Everything else renders as a plain note bullet.
pub fn is_todo(note: &ScratchpadNoteView) -> bool {
    note.note_type.eq_ignore_ascii_case("todo")
}

/// A tidy display label for a note's free-text `note_type`: the first letter
/// upper-cased, the rest lower-cased, with an **empty** type falling back to
/// `"Note"` (the daemon's default category). Keeps group headers consistent
/// without trusting the raw string's casing (`"todo"`/`"TODO"` → `"Todo"`).
pub fn type_label(note_type: &str) -> String {
    let mut chars = note_type.chars();
    match chars.next() {
        // Empty type → the daemon's default category name.
        None => "Note".to_string(),
        Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
    }
}

/// The checkbox glyph for a to-do row: a ballot-box-with-check when done, an
/// empty ballot box otherwise. Decorative — the row text carries the meaning.
pub fn checkbox_glyph(done: bool) -> &'static str {
    if done {
        "\u{2611}" // ☑ ballot box with check
    } else {
        "\u{2610}" // ☐ ballot box
    }
}

/// Group notes by `note_type`, **preserving the order** the daemon returned
/// (already sorted by type then sequence), collapsing every note of a type
/// under a single header even if the types happen to interleave. Returns
/// `(display_label, notes)` pairs in first-appearance order; empty input yields
/// an empty vec so the panel can show its empty state.
pub fn group_notes(notes: &[ScratchpadNoteView]) -> Vec<(String, Vec<ScratchpadNoteView>)> {
    let mut groups: Vec<(String, Vec<ScratchpadNoteView>)> = Vec::new();
    for note in notes {
        let label = type_label(&note.note_type);
        match groups.iter_mut().find(|(l, _)| l == &label) {
            Some((_, items)) => items.push(note.clone()),
            None => groups.push((label, vec![note.clone()])),
        }
    }
    groups
}

/// A one-line summary for the panel header: the note count (singular/plural),
/// plus a `"· done of total done"` progress hint when the pad holds any
/// to-dos. An empty pad returns an inviting empty-state line instead of a bare
/// `"0 notes"`.
pub fn summary(notes: &[ScratchpadNoteView]) -> String {
    if notes.is_empty() {
        return EMPTY_SUMMARY.to_string();
    }
    let total = notes.len();
    let count = if total == 1 {
        "1 note".to_string()
    } else {
        format!("{total} notes")
    };
    let todos: Vec<&ScratchpadNoteView> = notes.iter().filter(|n| is_todo(n)).collect();
    if todos.is_empty() {
        count
    } else {
        let done = todos.iter().filter(|n| n.done).count();
        format!("{count} \u{00b7} {done} of {} done", todos.len())
    }
}

/// The empty-pad summary line, shared by [`summary`] and the panel's empty
/// state so the two never drift.
pub const EMPTY_SUMMARY: &str = "Empty — Adele writes working notes here as she plans and works.";

/// The Leptos scratchpad panel (issue #16). Re-exported from the wasm-only
/// [`ui`] submodule; `settings.rs` renders it as the `Scratchpad` panel body.
#[cfg(target_arch = "wasm32")]
pub use ui::scratchpad_panel;

#[cfg(target_arch = "wasm32")]
mod ui {
    //! Mobile-first Leptos view: notes grouped by type, each group a header + a
    //! stack of rows. To-dos render with a (read-only) checkbox glyph and a
    //! strike-through when done; other notes render with a bullet. The pad is
    //! driven entirely by the shared reducer's `SidePaneSetScratchpad` /
    //! `FetchScratchpad` effects (mirrored into `view.scratchpad` by the engine),
    //! so it stays current turn-by-turn; a Refresh button pulls on demand.
    //!
    //! Read-only v1: note *content* is assistant-produced, so it is rendered as
    //! escaped plain text (Leptos escapes text children) — never `inner_html`.

    use leptos::prelude::*;

    use desktop_assistant_api_model::ScratchpadNoteView;

    use super::{EMPTY_SUMMARY, checkbox_glyph, group_notes, is_todo, summary};
    use crate::engine::ViewSignals;
    use crate::settings::EngineHandle;

    /// The panel body. Renders `view.scratchpad` (kept fresh by the reducer) and
    /// offers a Refresh button that re-reads the active conversation's pad.
    pub fn scratchpad_panel(engine: EngineHandle, view: ViewSignals) -> impl IntoView {
        let refresh = move |_| engine.with_value(|e| e.borrow().refresh_scratchpad());

        view! {
            <section class="panel scratchpad-panel">
                <div class="panel-intro">
                    <p class="panel-summary">{move || summary(&view.scratchpad.get())}</p>
                    <p class="panel-note muted">
                        "Adele's working notes for this conversation — her todos, findings, and \
                         decisions. She updates them as the conversation goes; read-only here \
                         for now."
                    </p>
                </div>

                <div class="field-head">
                    <span class="field-label">"Notes"</span>
                    <button class="link" on:click=refresh>
                        "Refresh"
                    </button>
                </div>

                {move || {
                    let notes = view.scratchpad.get();
                    if notes.is_empty() {
                        view! { <p class="empty muted">{EMPTY_SUMMARY}</p> }.into_any()
                    } else {
                        group_notes(&notes)
                            .into_iter()
                            .map(|(label, items)| note_group(label, items))
                            .collect_view()
                            .into_any()
                    }
                }}
            </section>
        }
    }

    /// One note-type group: a header plus its rows.
    fn note_group(label: String, items: Vec<ScratchpadNoteView>) -> impl IntoView {
        let rows = items.into_iter().map(note_row).collect_view();
        view! {
            <div class="scratchpad-group">
                <h3 class="group-header">{label}</h3>
                {rows}
            </div>
        }
    }

    /// A single note row. To-dos lead with a checkbox glyph and strike through
    /// their content when done; other notes lead with a bullet. The `key` (the
    /// LLM's stable handle for the note) trails as a subtle tag.
    fn note_row(note: ScratchpadNoteView) -> impl IntoView {
        let todo = is_todo(&note);
        let struck = todo && note.done;
        let marker = if todo {
            checkbox_glyph(note.done)
        } else {
            "\u{2022}" // • bullet
        };
        view! {
            <div class="scratchpad-note" class:done=struck>
                <span class="note-marker" aria-hidden="true">
                    {marker}
                </span>
                <div class="note-body">
                    <p class="note-content" class:struck=struck>
                        {note.content}
                    </p>
                    <span class="note-key muted">{note.key}</span>
                </div>
            </div>
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(key: &str, content: &str, note_type: &str, done: bool) -> ScratchpadNoteView {
        ScratchpadNoteView {
            id: format!("id-{key}"),
            key: key.to_string(),
            content: content.to_string(),
            note_type: note_type.to_string(),
            sequence: None,
            done,
            updated_at: "2026-07-14 00:00:00".to_string(),
        }
    }

    #[test]
    fn is_todo_matches_type_case_insensitively() {
        assert!(is_todo(&note("k", "c", "todo", false)));
        assert!(is_todo(&note("k", "c", "TODO", false)));
        assert!(is_todo(&note("k", "c", "ToDo", true)));
        assert!(!is_todo(&note("k", "c", "note", false)));
        assert!(!is_todo(&note("k", "c", "", false)));
        assert!(!is_todo(&note("k", "c", "other", false)));
    }

    #[test]
    fn type_label_capitalizes_and_defaults_empty_to_note() {
        assert_eq!(type_label(""), "Note");
        assert_eq!(type_label("note"), "Note");
        assert_eq!(type_label("todo"), "Todo");
        assert_eq!(type_label("TODO"), "Todo");
        assert_eq!(type_label("other"), "Other");
    }

    #[test]
    fn checkbox_glyph_reflects_done() {
        assert_ne!(
            checkbox_glyph(true),
            checkbox_glyph(false),
            "done and open todos must render distinct glyphs"
        );
        assert!(!checkbox_glyph(true).is_empty());
        assert!(!checkbox_glyph(false).is_empty());
    }

    #[test]
    fn group_notes_empty_is_empty() {
        assert!(group_notes(&[]).is_empty());
    }

    #[test]
    fn group_notes_groups_by_type_preserving_order() {
        let notes = vec![
            note("t1", "first", "todo", false),
            note("t2", "second", "todo", true),
            note("n1", "a note", "note", false),
        ];
        let groups = group_notes(&notes);
        assert_eq!(groups.len(), 2, "two distinct types → two groups");
        assert_eq!(groups[0].0, "Todo");
        assert_eq!(
            groups[0]
                .1
                .iter()
                .map(|n| n.key.as_str())
                .collect::<Vec<_>>(),
            vec!["t1", "t2"],
            "todo order (by sequence) is preserved"
        );
        assert_eq!(groups[1].0, "Note");
        assert_eq!(groups[1].1.len(), 1);
        assert_eq!(groups[1].1[0].key, "n1");
    }

    #[test]
    fn group_notes_collapses_interleaved_types_under_one_header() {
        // A defensive case: even if types interleave, each type collapses to a
        // single group in first-appearance order (no duplicate headers).
        let notes = vec![
            note("t1", "todo one", "todo", false),
            note("n1", "note one", "note", false),
            note("t2", "todo two", "todo", true),
        ];
        let groups = group_notes(&notes);
        assert_eq!(groups.len(), 2, "todo + note, not three groups");
        assert_eq!(groups[0].0, "Todo");
        assert_eq!(
            groups[0]
                .1
                .iter()
                .map(|n| n.key.as_str())
                .collect::<Vec<_>>(),
            vec!["t1", "t2"],
            "both todos land under the one Todo header, in order"
        );
        assert_eq!(groups[1].0, "Note");
        assert_eq!(groups[1].1[0].key, "n1");
    }

    #[test]
    fn summary_empty_pad_invites() {
        assert_eq!(summary(&[]), EMPTY_SUMMARY);
    }

    #[test]
    fn summary_counts_notes_singular_and_plural() {
        assert_eq!(summary(&[note("n1", "one", "note", false)]), "1 note");
        assert_eq!(
            summary(&[
                note("n1", "one", "note", false),
                note("n2", "two", "note", false),
            ]),
            "2 notes"
        );
    }

    #[test]
    fn summary_folds_todo_progress() {
        // 3 notes total, of which two are todos (one done) → count + progress.
        let notes = vec![
            note("t1", "todo one", "todo", true),
            note("t2", "todo two", "todo", false),
            note("n1", "a note", "note", false),
        ];
        assert_eq!(summary(&notes), "3 notes \u{00b7} 1 of 2 done");
    }

    #[test]
    fn summary_without_todos_has_no_progress_hint() {
        let notes = vec![
            note("n1", "one", "note", false),
            note("n2", "two", "context", false),
        ];
        let s = summary(&notes);
        assert_eq!(s, "2 notes");
        assert!(!s.contains("done"), "no todos → no progress hint");
    }
}
