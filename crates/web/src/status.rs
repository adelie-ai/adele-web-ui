//! The chat screen's status line (issue #73).
//!
//! The shared reducer routes every user-facing status string through
//! `Effect::SetStatusText` / `Effect::SetChatStatus`: provider errors
//! (`"Error: …"`), disconnects (`"Disconnected: …"`), send failures, the
//! connection label, and the per-turn progress line ("Searching knowledge
//! base…"). The engine mirrors them into
//! [`ViewSignals::status`](crate::engine::ViewSignals); this module turns that
//! raw string into a render-ready line, and the
//! `#[cfg(target_arch = "wasm32")]` view at the bottom paints it above the
//! composer — the web analogue of the GTK status label and the TUI status row.
//!
//! Kept transport- and view-free so it unit-tests on the host target like
//! [`crate::context`] / [`crate::model`].

/// The prefixes the shared reducer stamps on a status the user must not miss:
/// `UiMessage::Error` and a failed stream become `"Error: {text}"`, a dropped
/// connection becomes `"Disconnected: {reason}"`. Everything else — the
/// connection label and the per-turn progress line — is ordinary chatter.
const FAILURE_PREFIXES: [&str; 2] = ["Error:", "Disconnected:"];

/// How loudly a status string is presented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    /// Ordinary chatter: the connection label, "Connecting…", or a turn's
    /// progress line. Announced politely so it never interrupts a screen
    /// reader mid-sentence.
    Progress,
    /// A failure the user must see: a provider error, a context overflow, a
    /// send that did not go out, a dropped connection. Announced assertively.
    Failure,
}

impl StatusKind {
    /// CSS modifier appended to the `status-line` class.
    pub fn css_class(self) -> &'static str {
        match self {
            StatusKind::Progress => "status-progress",
            StatusKind::Failure => "status-failure",
        }
    }

    /// ARIA role for the rendered line.
    pub fn aria_role(self) -> &'static str {
        match self {
            StatusKind::Progress => "status",
            StatusKind::Failure => "alert",
        }
    }

    /// ARIA live-region politeness for the rendered line.
    pub fn aria_live(self) -> &'static str {
        match self {
            StatusKind::Progress => "polite",
            StatusKind::Failure => "assertive",
        }
    }
}

/// A status string ready to paint: the text to show and how loudly to show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusLine {
    /// The message, verbatim apart from trimmed surrounding whitespace. Never
    /// shortened or reworded — a truncated provider error is a lost diagnosis.
    pub text: String,
    pub kind: StatusKind,
}

/// Turn the engine's raw status string into a line to render, or `None` when
/// there is nothing to say (the line collapses to zero height).
///
/// Classification keys off the reducer's own [`FAILURE_PREFIXES`] rather than
/// searching for the word "error" anywhere in the string, so a progress line
/// that merely names a file stays calm.
pub fn status_line(text: &str) -> Option<StatusLine> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let kind = if FAILURE_PREFIXES
        .iter()
        .any(|prefix| text.starts_with(prefix))
    {
        StatusKind::Failure
    } else {
        StatusKind::Progress
    };
    Some(StatusLine {
        text: text.to_string(),
        kind,
    })
}

#[cfg(target_arch = "wasm32")]
pub use view::status_view;

#[cfg(target_arch = "wasm32")]
mod view {
    use leptos::prelude::*;

    use super::status_line;
    use crate::engine::ViewSignals;

    /// The status line, painted just above the composer — the last thing the
    /// user sees before their draft, so a turn that failed says why instead of
    /// ending in a bubble that silently disappears.
    ///
    /// Hidden (zero footprint) whenever there is nothing to say, so it never
    /// crowds a phone viewport. A failure is an assertive `alert`; progress is
    /// a polite `status`, so a screen reader is not interrupted every time a
    /// tool loop reports where it has got to.
    ///
    /// The text is rendered as a DOM **text node**, never `inner_html`: it can
    /// carry provider- or model-influenced content, and Leptos escapes it. This
    /// is deliberately unlike the chat bubbles, which go through
    /// [`crate::markdown`]'s sanitizer because they must render markup.
    pub fn status_view(view: ViewSignals) -> impl IntoView {
        move || {
            status_line(&view.status.get()).map(|line| {
                view! {
                    <p
                        class=format!("status-line {}", line.kind.css_class())
                        role=line.kind.aria_role()
                        aria-live=line.kind.aria_live()
                    >
                        {line.text}
                    </p>
                }
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_line_is_hidden_when_there_is_nothing_to_say() {
        assert_eq!(status_line(""), None);
    }

    #[test]
    fn status_line_is_hidden_for_a_whitespace_only_status() {
        // `Effect::ClearChatStatus` writes an empty string; a stray blank must
        // not paint an empty bar above the composer either.
        assert_eq!(status_line("   \n\t "), None);
    }

    #[test]
    fn provider_error_status_is_classified_as_a_failure() {
        // The reducer's `UiMessage::Error` / `StreamError` shape.
        let line = status_line("Error: 429 Too Many Requests").expect("an error must be shown");
        assert_eq!(line.kind, StatusKind::Failure);
        assert_eq!(line.text, "Error: 429 Too Many Requests");
    }

    #[test]
    fn disconnect_status_is_classified_as_a_failure() {
        let line = status_line("Disconnected: socket closed").expect("a disconnect must be shown");
        assert_eq!(line.kind, StatusKind::Failure);
    }

    #[test]
    fn send_failure_status_is_classified_as_a_failure() {
        // The engine's own offline-send path (`spawn_send`), which goes out as
        // `UiMessage::Error` and so arrives prefixed.
        let line = status_line("Error: Not connected — message not sent (your text is preserved).")
            .expect("a failed send must be shown");
        assert_eq!(line.kind, StatusKind::Failure);
    }

    #[test]
    fn turn_progress_status_is_classified_as_progress() {
        let line = status_line("Searching knowledge base…").expect("progress must be shown");
        assert_eq!(line.kind, StatusKind::Progress);
        assert_eq!(line.text, "Searching knowledge base…");
    }

    #[test]
    fn connection_label_status_is_classified_as_progress() {
        assert_eq!(
            status_line("Local daemon").map(|l| l.kind),
            Some(StatusKind::Progress)
        );
        assert_eq!(
            status_line("Connecting…").map(|l| l.kind),
            Some(StatusKind::Progress)
        );
    }

    #[test]
    fn status_that_merely_mentions_an_error_is_not_a_failure() {
        // Classification keys off the reducer's own prefix, not a substring
        // search — a progress line that happens to name a file must stay calm.
        assert_eq!(
            status_line("Reading ErrorLog.txt").map(|l| l.kind),
            Some(StatusKind::Progress)
        );
        assert_eq!(
            status_line("no error so far").map(|l| l.kind),
            Some(StatusKind::Progress)
        );
    }

    #[test]
    fn status_line_preserves_the_message_verbatim() {
        // A long provider error is shown whole: truncating it would hide the
        // reason the turn died. Markup is preserved too — the view renders the
        // text as a DOM text node (escaped), never as HTML, so nothing here
        // needs to strip it.
        let raw = "Error: model \"gpt-5.4\" not found <script>alert(1)</script> \
                   (connection bedrock-us-east-1, request 8f2c…)";
        let line = status_line(raw).expect("a long error must be shown");
        assert_eq!(line.text, raw);
    }

    #[test]
    fn status_line_trims_surrounding_whitespace() {
        let line = status_line("  Searching knowledge base…\n").expect("progress must be shown");
        assert_eq!(line.text, "Searching knowledge base…");
    }

    #[test]
    fn failure_status_is_announced_assertively() {
        assert_eq!(StatusKind::Failure.aria_role(), "alert");
        assert_eq!(StatusKind::Failure.aria_live(), "assertive");
        assert_eq!(StatusKind::Failure.css_class(), "status-failure");
    }

    #[test]
    fn progress_status_is_announced_politely() {
        assert_eq!(StatusKind::Progress.aria_role(), "status");
        assert_eq!(StatusKind::Progress.aria_live(), "polite");
        assert_eq!(StatusKind::Progress.css_class(), "status-progress");
    }
}
