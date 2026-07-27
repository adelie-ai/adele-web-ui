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
        todo!("issue #73: CSS modifier for {self:?}")
    }

    /// ARIA role for the rendered line.
    pub fn aria_role(self) -> &'static str {
        todo!("issue #73: ARIA role for {self:?}")
    }

    /// ARIA live-region politeness for the rendered line.
    pub fn aria_live(self) -> &'static str {
        todo!("issue #73: ARIA politeness for {self:?}")
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
pub fn status_line(text: &str) -> Option<StatusLine> {
    todo!("issue #73: classify and surface {text:?}")
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
