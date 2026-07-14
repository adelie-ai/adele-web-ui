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
    /// The `request_id` of the last reply spoken aloud, so a re-delivery of the
    /// same completion is not spoken twice. Reset on a conversation change.
    last_spoken_request: Option<String>,
}

impl ReadAloud {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decide what to speak when an assistant reply *completes*. Speaks the reply
    /// iff the toggle is `enabled`, the text is not blank, and this `request_id`
    /// has not already been spoken (recording it so a re-delivered `StreamComplete`
    /// — a cross-client echo of the same turn — is not spoken again). A reply seen
    /// while disabled is deliberately *not* recorded, so it can still be spoken if
    /// re-evaluated once enabled.
    pub fn on_completed_reply(
        &mut self,
        enabled: bool,
        request_id: &str,
        text: &str,
    ) -> SpeechAction {
        if !enabled {
            return SpeechAction::Silent;
        }
        if self.last_spoken_request.as_deref() == Some(request_id) {
            return SpeechAction::Silent;
        }
        if text.trim().is_empty() {
            return SpeechAction::Silent;
        }
        self.last_spoken_request = Some(request_id.to_string());
        SpeechAction::Speak(text.to_string())
    }

    /// Decide what to do when the toggle is flipped. Turning it *off* cancels any
    /// speech in progress; turning it *on* is silent (it never retro-speaks an
    /// already-completed reply — the speak path only fires on a *new* completion).
    pub fn on_toggle(&self, now_enabled: bool) -> SpeechAction {
        if now_enabled {
            SpeechAction::Silent
        } else {
            SpeechAction::Cancel
        }
    }

    /// Decide what to do when the open conversation changes. Speech from the
    /// previous conversation is cancelled and the dedup state is reset so the new
    /// conversation starts clean.
    pub fn on_conversation_change(&mut self) -> SpeechAction {
        self.last_spoken_request = None;
        SpeechAction::Cancel
    }
}

#[cfg(target_arch = "wasm32")]
pub use view::read_aloud_toggle;

#[cfg(target_arch = "wasm32")]
mod view {
    use std::cell::RefCell;

    use leptos::prelude::*;

    use super::{ReadAloud, SpeechAction};
    use crate::engine::ViewSignals;

    /// The header read-aloud control. **Capability-detected**: when the browser
    /// has no `speechSynthesis` it renders nothing (`Option<_>: IntoView`), so the
    /// toggle is simply absent rather than a dead control. Otherwise it wires the
    /// toggle to the pure [`ReadAloud`] core and the browser synthesizer.
    pub fn read_aloud_toggle(view: ViewSignals) -> impl IntoView {
        synth::available().then(|| toggle(view))
    }

    fn toggle(view: ViewSignals) -> impl IntoView {
        let enabled = RwSignal::new(false);
        // The dedup core, shared (as a `Copy` handle) by the two effects and the
        // click handler — all on the single CSR thread, so a `RefCell` suffices.
        let core = StoredValue::new_local(RefCell::new(ReadAloud::new()));

        // Speak each newly-completed assistant reply while the toggle is on. Reads
        // `last_completed_reply` reactively (set by the engine on `StreamComplete`)
        // but `enabled` untracked, so flipping the toggle never retro-speaks the
        // last reply — only a genuinely new completion drives this.
        Effect::new(move |_| {
            let Some((request_id, text)) = view.last_completed_reply.get() else {
                return;
            };
            let action = core.with_value(|c| {
                c.borrow_mut()
                    .on_completed_reply(enabled.get_untracked(), &request_id, &text)
            });
            apply(action);
        });

        // Switching the open conversation stops any speech from the previous one.
        // `current_conversation_id` is re-set every dispatch, so guard on an actual
        // change (and skip the initial `None -> Some` load, which has nothing to
        // cancel) to avoid churning cancels.
        Effect::new(move |prev: Option<Option<String>>| {
            let current = view.current_conversation_id.get();
            if let Some(previous) = prev
                && previous.is_some()
                && previous != current
            {
                apply(core.with_value(|c| c.borrow_mut().on_conversation_change()));
            }
            current
        });

        let on_click = move |_| {
            let now = !enabled.get_untracked();
            enabled.set(now);
            apply(core.with_value(|c| c.borrow().on_toggle(now)));
        };

        view! {
            <button
                class="read-aloud-toggle icon-btn"
                class:active=move || enabled.get()
                aria-label="Read replies aloud"
                aria-pressed=move || if enabled.get() { "true" } else { "false" }
                on:click=on_click
            >
                {move || if enabled.get() { "\u{1f50a}" } else { "\u{1f507}" }}
            </button>
        }
    }

    /// Carry out a [`SpeechAction`] against the browser synthesizer.
    fn apply(action: SpeechAction) {
        match action {
            SpeechAction::Speak(text) => synth::speak(&text),
            SpeechAction::Cancel => synth::cancel(),
            SpeechAction::Silent => {}
        }
    }

    /// The browser `SpeechSynthesis` glue, isolated so the rest of the module is
    /// DOM-free. Capability detection lives here too.
    mod synth {
        use wasm_bindgen::JsValue;
        use web_sys::{SpeechSynthesis, SpeechSynthesisUtterance};

        /// The browser speech synthesizer, if genuinely present. `speechSynthesis`
        /// is bound as a `catch` getter, which wraps a *missing* property as a
        /// value over `undefined` rather than an `Err`; verify the object is real
        /// before handing it back, so capability detection is honest.
        fn get() -> Option<SpeechSynthesis> {
            let synth = web_sys::window()?.speech_synthesis().ok()?;
            let value: &JsValue = synth.as_ref();
            (!value.is_undefined() && !value.is_null()).then_some(synth)
        }

        pub fn available() -> bool {
            get().is_some()
        }

        pub fn speak(text: &str) {
            if let Some(synth) = get()
                && let Ok(utterance) = SpeechSynthesisUtterance::new_with_text(text)
            {
                synth.speak(&utterance);
            }
        }

        pub fn cancel() {
            if let Some(synth) = get() {
                synth.cancel();
            }
        }
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
        assert_eq!(
            ra.on_completed_reply(true, "r1", "one"),
            SpeechAction::Silent
        );
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
        assert_eq!(
            ra.on_completed_reply(false, "r1", "hi"),
            SpeechAction::Silent
        );
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
