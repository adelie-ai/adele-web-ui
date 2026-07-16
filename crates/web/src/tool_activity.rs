//! The optional "show tool activity" transcript view (#59), kept
//! transport-/Leptos-free in its core so it compiles and unit-tests on the host
//! target (like [`crate::wire`] / [`crate::model`] / [`crate::reply`]).
//!
//! Tool results are display noise by default and are stripped server-side by the
//! BFF (#58). When the user opts in (a per-device toggle persisted in
//! localStorage), the SPA fetches the conversation's **full** message history
//! (all roles) via the daemon's existing `GetMessages` command and interleaves
//! the tool rows into the *live* transcript.
//!
//! Placement is **by position, not id**: live bubbles are finalized with empty
//! ids (the reducer only reconciles real ids on reload), so a tool row is placed
//! by how many display bubbles precede it in the fetched snapshot, then spliced
//! into the live bubble list at that ordinal. The user/assistant bubbles stay
//! reducer-owned and live (cross-client sync keeps working); only the historical
//! tool results come from the snapshot. With an empty snapshot the output is
//! exactly the live bubble list.

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

/// Does a snapshot message count as a display bubble (so tool rows can be placed
/// relative to it)? Matches the BFF / `client-ui-common` filter: `user` turns and
/// `assistant` turns with visible text. Empty tool-call-only assistant turns and
/// `system` messages are not display content.
fn is_display_bubble(role: &str, content: &str) -> bool {
    matches!(role, "user") || (role == "assistant" && !content.trim().is_empty())
}

/// Interleave the conversation's tool results into the live transcript.
///
/// `bubbles` is the live display list (user/assistant `(role, content)` pairs,
/// reducer-owned, rendered verbatim). `snapshot` is the full ordered history
/// (all roles) fetched via `GetMessages`. Each `tool` row in the snapshot is
/// placed after the number of display bubbles that precede it there, then spliced
/// into `bubbles` at that ordinal — so tool results land under the turn that ran
/// them without depending on message ids (the live bubbles have none until a
/// reload). `system` and empty tool-call-only assistant snapshot rows are skipped
/// for counting and never rendered; empty tool rows are dropped. With `snapshot`
/// empty (the toggle off, or nothing fetched yet) the result is exactly `bubbles`.
pub fn interleave_tool_rows(
    bubbles: Vec<(String, String)>,
    snapshot: Vec<(String, String)>,
) -> Vec<TranscriptItem> {
    // tools_after[k] = tool results that follow exactly k display bubbles.
    let mut tools_after: Vec<Vec<String>> = vec![Vec::new(); bubbles.len() + 1];
    let mut seen = 0usize;
    for (role, content) in &snapshot {
        if is_display_bubble(role, content) {
            seen += 1;
        } else if role == "tool" && !content.trim().is_empty() {
            // Clamp so a tool row past the live tail (e.g. the snapshot is a turn
            // ahead of the not-yet-updated bubbles) renders at the end rather than
            // out of bounds.
            let k = seen.min(bubbles.len());
            tools_after[k].push(content.clone());
        }
    }

    let mut out = Vec::with_capacity(bubbles.len());
    for (k, tools) in tools_after.iter().enumerate() {
        for t in tools {
            out.push(TranscriptItem::ToolActivity {
                preview: tool_preview(t),
                full: t.clone(),
            });
        }
        if let Some((role, content)) = bubbles.get(k) {
            out.push(TranscriptItem::Bubble {
                role: role.clone(),
                content: content.clone(),
            });
        }
    }
    out
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

/// Header toggle for the "show tool activity" view: flips the per-device signal
/// and persists it. The transcript re-derives reactively; a watching effect in
/// `app` fetches the snapshot when it turns on.
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

/// The chat transcript rows: live user/assistant bubbles, with tool results
/// interleaved as collapsed `<details>` when the opt-in is on. With the toggle
/// off the snapshot is empty and this is exactly the live bubble list.
#[cfg(target_arch = "wasm32")]
pub fn transcript_view(view: crate::engine::ViewSignals) -> impl leptos::IntoView {
    use leptos::prelude::*;
    move || {
        let bubbles: Vec<(String, String)> = view
            .messages
            .get()
            .into_iter()
            .map(|m| (m.role, m.content))
            .collect();
        let snapshot: Vec<(String, String)> = if view.show_tool_activity.get() {
            view.tool_activity
                .get()
                .into_iter()
                .map(|m| (m.role, m.content))
                .collect()
        } else {
            Vec::new()
        };
        interleave_tool_rows(bubbles, snapshot)
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

    fn p(role: &str, content: &str) -> (String, String) {
        (role.to_string(), content.to_string())
    }

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
    fn empty_snapshot_is_just_the_live_bubbles() {
        let bubbles = vec![p("user", "drive?"), p("assistant", "~40h")];
        let out = interleave_tool_rows(bubbles, vec![]);
        assert_eq!(
            out,
            vec![bubble("user", "drive?"), bubble("assistant", "~40h")]
        );
    }

    #[test]
    fn tool_placed_after_its_turn() {
        // Snapshot: user, empty tool-call assistant (dropped), tool, final answer.
        let bubbles = vec![p("user", "drive?"), p("assistant", "~40h")];
        let snapshot = vec![
            p("user", "drive?"),
            p("assistant", "   "),
            p("tool", r#"{"distance_m":4300000}"#),
            p("assistant", "~40h"),
        ];
        let out = interleave_tool_rows(bubbles, snapshot);
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
    fn multiple_tools_across_multiple_turns() {
        let bubbles = vec![
            p("user", "q1"),
            p("assistant", "a1"),
            p("user", "q2"),
            p("assistant", "a2"),
        ];
        let snapshot = vec![
            p("user", "q1"),
            p("tool", "t1"),
            p("assistant", "a1"),
            p("user", "q2"),
            p("tool", "t2"),
            p("assistant", "a2"),
        ];
        let out = interleave_tool_rows(bubbles, snapshot);
        assert_eq!(
            kinds(&out),
            vec!["bubble", "tool", "bubble", "bubble", "tool", "bubble"]
        );
        // Each tool sits under the turn that ran it, not clustered at the top —
        // the exact failure the id-merge approach had with empty-id live bubbles.
        assert_eq!(out[0], bubble("user", "q1"));
        assert_eq!(out[2], bubble("assistant", "a1"));
        assert_eq!(out[3], bubble("user", "q2"));
        assert_eq!(out[5], bubble("assistant", "a2"));
    }

    #[test]
    fn tool_before_any_bubble_renders_first() {
        let bubbles = vec![p("user", "q")];
        let snapshot = vec![p("tool", "early"), p("user", "q")];
        let out = interleave_tool_rows(bubbles, snapshot);
        assert_eq!(kinds(&out), vec!["tool", "bubble"]);
    }

    #[test]
    fn system_and_empty_tool_rows_are_skipped() {
        let bubbles = vec![p("user", "hi"), p("assistant", "hello")];
        let snapshot = vec![
            p("system", "You are Adele."),
            p("user", "hi"),
            p("tool", "   "),
            p("assistant", "hello"),
        ];
        let out = interleave_tool_rows(bubbles, snapshot);
        assert_eq!(
            out,
            vec![bubble("user", "hi"), bubble("assistant", "hello")]
        );
    }

    #[test]
    fn tool_past_live_tail_clamps_to_end() {
        // The snapshot is a turn ahead of the not-yet-updated live bubbles: the
        // new tool row renders at the end rather than being dropped or panicking.
        let bubbles = vec![p("user", "q1")];
        let snapshot = vec![p("user", "q1"), p("assistant", "a1"), p("tool", "t1")];
        let out = interleave_tool_rows(bubbles, snapshot);
        assert_eq!(kinds(&out), vec!["bubble", "tool"]);
    }

    #[test]
    fn no_bubbles_still_renders_orphan_tool() {
        let out = interleave_tool_rows(vec![], vec![p("tool", "t")]);
        assert_eq!(kinds(&out), vec!["tool"]);
    }

    #[test]
    fn bubble_roles_are_preserved() {
        let out = interleave_tool_rows(vec![p("assistant", "text")], vec![]);
        assert_eq!(out, vec![bubble("assistant", "text")]);
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
    fn preview_at_exact_cap_is_not_truncated() {
        let exact = "x".repeat(PREVIEW_MAX_CHARS);
        assert_eq!(
            tool_preview(&exact),
            exact,
            "== cap is verbatim, no ellipsis"
        );
    }

    #[test]
    fn short_preview_is_verbatim_single_line() {
        assert_eq!(tool_preview("  ok  done  "), "ok done");
    }
}
