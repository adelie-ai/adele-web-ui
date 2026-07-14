//! Browser read-aloud (issue #18, v1).
//!
//! A per-conversation accessibility toggle that speaks each *completed*
//! assistant reply through the browser's [`SpeechSynthesis`] (Web Speech API).
//! It is deliberately a **local, browser-only** feature: no server audio, no
//! mic/STT (a much larger follow-up), and — crucially — no daemon change. The
//! daemon's own voice state (`speech_mode` / `say_this` / reply narration) is the
//! *native* clients' TTS channel: it lives in the shared reducer as the
//! `AdeleOutput` narration gates + [`Effect::Speak`](client_ui_common::Effect)
//! fed to an embedded `Speaker`, driven by the daemon's `[assistant] speech_mode`
//! config. None of that is a web `Command` the browser drives, so this toggle
//! stands on its own rather than being wired into those gates (which would couple
//! the web client to daemon voice config and risk double-speaking).
//!
//! This module keeps the **decision logic pure** ([`ReadAloud`] /
//! [`SpeechAction`]) so it unit-tests on the host target like [`crate::wire`] /
//! [`crate::context`]; the `#[cfg(target_arch = "wasm32")]` view at the bottom
//! owns the browser-`SpeechSynthesis` glue (including capability detection — the
//! toggle hides when the API is absent) and the Leptos header control.

/// What the browser speech layer should do in response to a UI/stream event.
/// Kept separate from the DOM so the decision is host-testable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpeechAction {
    /// Speak this text aloud.
    Speak(String),
    /// Cancel any in-flight or queued speech.
    Cancel,
    /// Do nothing.
    Silent,
}

/// Pure decision core for read-aloud. Holds only the small dedup state needed to
/// avoid speaking the same completed reply twice (e.g. a cross-client echo of the
/// same turn re-delivers its `StreamComplete`); everything else is a function of
/// its inputs. The `enabled` flag is passed in per call rather than stored, so
/// the reactive UI owns the single source of truth for the toggle.
#[derive(Default)]
pub struct ReadAloud {
    // Filled in by the implementing commit.
}

impl ReadAloud {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decide what to speak when an assistant reply *completes*. Speaks the reply
    /// iff the toggle is `enabled`, the text is not blank, and this `request_id`
    /// has not already been spoken. Stub: unimplemented (the spec commit).
    pub fn on_completed_reply(
        &mut self,
        _enabled: bool,
        _request_id: &str,
        _text: &str,
    ) -> SpeechAction {
        SpeechAction::Silent
    }

    /// Decide what to do when the toggle is flipped. Turning it *off* cancels any
    /// speech in progress; turning it *on* is silent (it never retro-speaks an
    /// already-completed reply). Stub: unimplemented (the spec commit).
    pub fn on_toggle(&self, _now_enabled: bool) -> SpeechAction {
        SpeechAction::Silent
    }

    /// Decide what to do when the open conversation changes. Speech from the
    /// previous conversation is cancelled and the dedup state is reset so the new
    /// conversation starts clean. Stub: unimplemented (the spec commit).
    pub fn on_conversation_change(&mut self) -> SpeechAction {
        SpeechAction::Silent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speaks_completed_reply_when_enabled() {
        let mut ra = ReadAloud::new();
        assert_eq!(
            ra.on_completed_reply(true, "r1", "Hello there."),
            SpeechAction::Speak("Hello there.".to_string())
        );
    }

    #[test]
    fn silent_when_disabled() {
        let mut ra = ReadAloud::new();
        assert_eq!(
            ra.on_completed_reply(false, "r1", "Hello there."),
            SpeechAction::Silent
        );
    }

    #[test]
    fn dedups_the_same_request_id() {
        // A completed reply speaks once; a re-delivery of the SAME turn's
        // completion (cross-client echo) must not speak it again.
        let mut ra = ReadAloud::new();
        assert_eq!(
            ra.on_completed_reply(true, "r1", "one"),
            SpeechAction::Speak("one".to_string())
        );
        assert_eq!(ra.on_completed_reply(true, "r1", "one"), SpeechAction::Silent);
    }

    #[test]
    fn speaks_each_distinct_reply() {
        let mut ra = ReadAloud::new();
        assert_eq!(
            ra.on_completed_reply(true, "r1", "one"),
            SpeechAction::Speak("one".to_string())
        );
        assert_eq!(
            ra.on_completed_reply(true, "r2", "two"),
            SpeechAction::Speak("two".to_string())
        );
    }

    #[test]
    fn blank_reply_is_silent() {
        let mut ra = ReadAloud::new();
        assert_eq!(
            ra.on_completed_reply(true, "r1", "   \n\t "),
            SpeechAction::Silent
        );
    }

    #[test]
    fn disabled_reply_is_not_consumed_by_dedup() {
        // A reply seen while disabled is NOT recorded, so if the same reply is
        // re-evaluated once enabled it still speaks (the toggle isn't a mute that
        // silently swallows the pending reply).
        let mut ra = ReadAloud::new();
        assert_eq!(ra.on_completed_reply(false, "r1", "hi"), SpeechAction::Silent);
        assert_eq!(
            ra.on_completed_reply(true, "r1", "hi"),
            SpeechAction::Speak("hi".to_string())
        );
    }

    #[test]
    fn toggling_off_cancels() {
        let ra = ReadAloud::new();
        assert_eq!(ra.on_toggle(false), SpeechAction::Cancel);
    }

    #[test]
    fn toggling_on_is_silent() {
        // Turning read-aloud on must not retro-speak the last completed reply.
        let ra = ReadAloud::new();
        assert_eq!(ra.on_toggle(true), SpeechAction::Silent);
    }

    #[test]
    fn conversation_change_cancels_and_resets_dedup() {
        let mut ra = ReadAloud::new();
        assert_eq!(
            ra.on_completed_reply(true, "r1", "one"),
            SpeechAction::Speak("one".to_string())
        );
        // Switching away cancels in-flight speech...
        assert_eq!(ra.on_conversation_change(), SpeechAction::Cancel);
        // ...and clears the dedup, so a reply carrying the same id (a fresh turn
        // in the newly-opened conversation) speaks again.
        assert_eq!(
            ra.on_completed_reply(true, "r1", "one"),
            SpeechAction::Speak("one".to_string())
        );
    }
}
