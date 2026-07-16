//! The optional "show tool activity" transcript view (#59), kept
//! transport-/Leptos-free in its core so it compiles and unit-tests on the host
//! target (like [`crate::wire`] / [`crate::model`] / [`crate::reply`]).
//!
//! Tool results are display noise by default and are stripped server-side by the
//! BFF (#58). When the user opts in (a per-device toggle persisted in
//! localStorage), the SPA fetches the conversation's tool rows via the daemon's
//! existing `GetMessages { include_roles: ["tool"] }` command and **interleaves**
//! them into the live transcript by message id — so the user/assistant bubbles
//! stay reducer-owned and live (cross-client sync keeps working) while historical
//! tool results appear collapsed in their true chronological place.
//!
//! [`build_verbose_transcript`] is the whole render model: default mode passes an
//! empty `tool_rows`, so it degrades to exactly the bubble list the transcript
//! shows today.

/// A message for transcript building: the UUIDv7 `id` (empty for an optimistic /
/// streaming tail not yet persisted), the `role`, and the `content`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MsgRef {
    pub id: String,
    pub role: String,
    pub content: String,
}

impl MsgRef {
    pub fn new(id: impl Into<String>, role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            role: role.into(),
            content: content.into(),
        }
    }
}

/// One rendered row of the chat transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptItem {
    /// A user or assistant turn shown as an ordinary chat bubble. `role` is the
    /// CSS role class (`"user"` / `"assistant"`); `content` is the markdown body.
    Bubble { role: String, content: String },
    /// A tool result, shown collapsed behind a `<details>`/`<summary>`. `preview`
    /// is a compact one-line summary for the closed state; `full` is the complete
    /// content revealed on expand.
    ToolActivity { preview: String, full: String },
}

/// Longest tool-result preview shown in the collapsed summary, in characters.
const PREVIEW_MAX_CHARS: usize = 80;

/// Build the transcript render model by merging the live `messages` (user /
/// assistant bubbles, reducer-owned) with separately-fetched `tool_rows` and
/// classifying each into a [`TranscriptItem`].
///
/// Merge order is by UUIDv7 `id` (chronological). Empty ids — an optimistic or
/// still-streaming tail not yet persisted — sort **last** so the live tail stays
/// at the bottom. The sort is stable, so same-id or same-empty entries keep their
/// input order (`messages` before `tool_rows`).
///
/// Classification matches the BFF / `client-ui-common` filter for bubbles: keep
/// `user` turns and `assistant` turns with visible text; render `tool` turns with
/// content as collapsed [`ToolActivity`]; drop empty tool-call-only assistant
/// turns, `system` messages, and empty tool rows. With `tool_rows` empty, the
/// output equals the default (bubbles-only) transcript.
pub fn build_verbose_transcript(
    messages: Vec<MsgRef>,
    tool_rows: Vec<MsgRef>,
) -> Vec<TranscriptItem> {
    let mut all = messages;
    all.extend(tool_rows);
    // Stable sort by (id-empty?, id): non-empty (false) before empty (true), then
    // lexical UUIDv7 order (chronological). Empty ids — the optimistic/streaming
    // tail — sort last; stable, so on an id tie a message keeps its place ahead of
    // a tool row (messages were appended first).
    all.sort_by(|a, b| (a.id.is_empty(), &a.id).cmp(&(b.id.is_empty(), &b.id)));
    all.into_iter()
        .filter_map(|m| classify(&m.role, &m.content))
        .collect()
}

/// Classify one `(role, content)` into a transcript row, or `None` when it is not
/// display content (empty assistant turn, `system`, empty tool result).
#[cfg_attr(not(test), allow(dead_code))]
fn classify(role: &str, content: &str) -> Option<TranscriptItem> {
    match role {
        "user" => Some(TranscriptItem::Bubble {
            role: "user".to_string(),
            content: content.to_string(),
        }),
        "assistant" if !content.trim().is_empty() => Some(TranscriptItem::Bubble {
            role: "assistant".to_string(),
            content: content.to_string(),
        }),
        "tool" if !content.trim().is_empty() => Some(TranscriptItem::ToolActivity {
            preview: tool_preview(content),
            full: content.to_string(),
        }),
        _ => None,
    }
}

/// A compact, single-line preview of a tool result for the collapsed summary:
/// whitespace collapsed, trimmed, and truncated with a plain-ASCII ellipsis.
#[cfg_attr(not(test), allow(dead_code))]
fn tool_preview(content: &str) -> String {
    let one_line = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() > PREVIEW_MAX_CHARS {
        let head: String = one_line.chars().take(PREVIEW_MAX_CHARS).collect();
        format!("{head}...")
    } else {
        one_line
    }
}

// --- Persistence + Leptos views (wasm only) ----------------------------------

/// localStorage key for the per-device "show tool activity" toggle.
#[cfg(target_arch = "wasm32")]
const TOGGLE_KEY: &str = "adele.show_tool_activity";

/// Read the persisted per-device toggle. Default off — tool activity is opt-in.
#[cfg(target_arch = "wasm32")]
pub fn load_persisted_toggle() -> bool {
    use gloo_storage::{LocalStorage, Storage};
    LocalStorage::get::<bool>(TOGGLE_KEY).unwrap_or(false)
}

/// Persist the per-device toggle so it survives reloads.
#[cfg(target_arch = "wasm32")]
fn persist_toggle(on: bool) {
    use gloo_storage::{LocalStorage, Storage};
    let _ = LocalStorage::set(TOGGLE_KEY, on);
}

/// Header toggle for the "show tool activity" view (#59): flips the per-device
/// signal and persists it. The transcript re-derives reactively; a watching
/// effect in `app` fetches the rows when it turns on.
#[cfg(target_arch = "wasm32")]
pub fn tool_activity_toggle(view: crate::engine::ViewSignals) -> impl leptos::IntoView {
    use leptos::prelude::*;
    let on_click = move |_| {
        let now = !view.show_tool_activity.get_untracked();
        view.show_tool_activity.set(now);
        persist_toggle(now);
    };
    view! {
        <button
            class="icon-btn tool-activity-toggle"
            class:active=move || view.show_tool_activity.get()
            aria-pressed=move || if view.show_tool_activity.get() { "true" } else { "false" }
            aria-label="Show tool activity"
            title="Show tool activity"
            on:click=on_click
        >
            "\u{1F527}"
        </button>
    }
}

/// The chat transcript rows: user/assistant bubbles, with tool results
/// interleaved as collapsed `<details>` when the opt-in is on. Shares one render
/// model with the default view via [`build_verbose_transcript`] — with the toggle
/// off, `tool_rows` is empty and this is exactly the bubble list.
#[cfg(target_arch = "wasm32")]
pub fn transcript_view(view: crate::engine::ViewSignals) -> impl leptos::IntoView {
    use leptos::prelude::*;
    move || {
        let messages: Vec<MsgRef> = view
            .messages
            .get()
            .iter()
            .map(|m| MsgRef::new(m.id.clone(), m.role.clone(), m.content.clone()))
            .collect();
        let tool_rows: Vec<MsgRef> = if view.show_tool_activity.get() {
            view.tool_activity
                .get()
                .iter()
                .map(|m| MsgRef::new(m.id.clone(), m.role.clone(), m.content.clone()))
                .collect()
        } else {
            Vec::new()
        };
        build_verbose_transcript(messages, tool_rows)
            .into_iter()
            .map(|item| match item {
                TranscriptItem::Bubble { role, content } => view! {
                    <div class=format!(
                        "msg {role}",
                    )>{crate::markdown::message_body(&content)}</div>
                }
                .into_any(),
                TranscriptItem::ToolActivity { preview, full } => view! {
                    <details class="msg tool-activity">
                        <summary>
                            <span class="tool-activity-label">"Tool result"</span>
                            <span class="tool-activity-preview">{preview}</span>
                        </summary>
                        <pre class="tool-activity-body">{full}</pre>
                    </details>
                }
                .into_any(),
            })
            .collect_view()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bubble(role: &str, content: &str) -> TranscriptItem {
        TranscriptItem::Bubble {
            role: role.to_string(),
            content: content.to_string(),
        }
    }

    fn kinds(items: &[TranscriptItem]) -> Vec<&str> {
        items
            .iter()
            .map(|i| match i {
                TranscriptItem::Bubble { .. } => "bubble",
                TranscriptItem::ToolActivity { .. } => "tool",
            })
            .collect()
    }

    #[test]
    fn bubbles_only_when_no_tool_rows() {
        let messages = vec![
            MsgRef::new("1", "user", "drive?"),
            MsgRef::new("2", "assistant", "~40h"),
        ];
        let out = build_verbose_transcript(messages, vec![]);
        assert_eq!(
            out,
            vec![bubble("user", "drive?"), bubble("assistant", "~40h")]
        );
    }

    #[test]
    fn interleaves_tool_rows_by_id() {
        // ids sort id1 < id2 < id3, so the tool row lands between the bubbles.
        let messages = vec![
            MsgRef::new("id1", "user", "drive?"),
            MsgRef::new("id3", "assistant", "~40h"),
        ];
        let tool_rows = vec![MsgRef::new("id2", "tool", r#"{"distance_m":4300000}"#)];
        let out = build_verbose_transcript(messages, tool_rows);
        assert_eq!(kinds(&out), vec!["bubble", "tool", "bubble"]);
        assert_eq!(out[0], bubble("user", "drive?"));
        assert_eq!(out[2], bubble("assistant", "~40h"));
        match &out[1] {
            TranscriptItem::ToolActivity { preview, full } => {
                assert_eq!(full, r#"{"distance_m":4300000}"#);
                assert_eq!(preview, r#"{"distance_m":4300000}"#);
            }
            other => panic!("expected ToolActivity, got {other:?}"),
        }
    }

    #[test]
    fn drops_empty_assistant_and_system() {
        let messages = vec![
            MsgRef::new("0", "system", "You are Adele."),
            MsgRef::new("1", "user", "hi"),
            MsgRef::new("2", "assistant", "   "),
            MsgRef::new("3", "assistant", "hello"),
        ];
        let out = build_verbose_transcript(messages, vec![]);
        assert_eq!(
            out,
            vec![bubble("user", "hi"), bubble("assistant", "hello")]
        );
    }

    #[test]
    fn empty_tool_content_is_not_shown() {
        let messages = vec![MsgRef::new("1", "user", "hi")];
        let tool_rows = vec![MsgRef::new("2", "tool", "   ")];
        let out = build_verbose_transcript(messages, tool_rows);
        assert_eq!(out, vec![bubble("user", "hi")]);
    }

    #[test]
    fn optimistic_empty_id_message_sorts_last() {
        // A just-sent user turn has no persisted id yet; it must stay at the
        // bottom even though a real-id tool row exists.
        let messages = vec![
            MsgRef::new("id1", "user", "first"),
            MsgRef::new("", "user", "just typed"),
        ];
        let tool_rows = vec![MsgRef::new("id2", "tool", "a-result")];
        let out = build_verbose_transcript(messages, tool_rows);
        assert_eq!(kinds(&out), vec!["bubble", "tool", "bubble"]);
        assert_eq!(out[0], bubble("user", "first"));
        assert_eq!(
            out[2],
            bubble("user", "just typed"),
            "empty-id tail stays last"
        );
    }

    #[test]
    fn stable_merge_keeps_message_before_tool_on_id_tie() {
        // Degenerate equal-id case: stable order keeps the message ahead of the
        // tool row (messages are appended first).
        let messages = vec![MsgRef::new("x", "assistant", "text")];
        let tool_rows = vec![MsgRef::new("x", "tool", "res")];
        let out = build_verbose_transcript(messages, tool_rows);
        assert_eq!(kinds(&out), vec!["bubble", "tool"]);
    }

    #[test]
    fn empty_inputs_are_empty() {
        assert!(build_verbose_transcript(vec![], vec![]).is_empty());
    }

    #[test]
    fn preview_is_single_line_and_truncated() {
        let noisy = format!("line one\n  line two\t{}", "x".repeat(200));
        let p = tool_preview(&noisy);
        assert!(
            !p.contains('\n') && !p.contains('\t'),
            "collapsed to one line"
        );
        assert!(p.ends_with("..."), "truncated with plain-ascii ellipsis");
        assert_eq!(p.chars().count(), PREVIEW_MAX_CHARS + 3, "capped + '...'");
    }

    #[test]
    fn short_preview_is_verbatim_single_line() {
        assert_eq!(tool_preview("  ok  done  "), "ok done");
    }
}
