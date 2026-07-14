//! The Purposes settings panel (issue #11): route each LLM *purpose* to a
//! connection + model (+ optional effort), or inherit the interactive purpose.
//!
//! Six purposes are rendered — `interactive, dreaming, consolidation, embedding,
//! titling, voice`. Unlike the TUI (which omits `voice`), the web client shows
//! all six, driven by [`PurposeKindApi::all`].
//!
//! **`"primary"` = inherit.** A non-interactive purpose whose `connection` *and*
//! `model` are the sentinel string `"primary"` inherits the interactive
//! purpose's LLM. The panel surfaces this as an explicit "Inherit from
//! Interactive" option rather than a literal string the user must type. The
//! interactive purpose itself cannot inherit (there is nothing to inherit from).
//!
//! **Split like `model.rs`/`wire.rs`.** The pure slot ↔ [`PurposeConfigView`]
//! mapping and validation live here and unit-test on the host target; the Leptos
//! panel is a `#[cfg(target_arch = "wasm32")]` submodule below.

use desktop_assistant_api_model::{
    ConnectionView, EffortLevel, ModelListing, PurposeConfigView, PurposeKindApi, PurposesView,
};

/// Sentinel meaning "inherit the interactive purpose" in a config's
/// `connection`/`model`. Must match the daemon's contract (`crates/daemon/src/purposes.rs`).
pub const PRIMARY: &str = "primary";

/// The slot in a [`PurposesView`] for `purpose`. `None` = unconfigured (the
/// daemon falls back to the interactive purpose). Covers all six kinds,
/// including `voice`.
pub fn slot_config(view: &PurposesView, purpose: PurposeKindApi) -> Option<&PurposeConfigView> {
    let _ = (view, purpose);
    None
}

/// Human-readable name for a purpose (title-cased). Defined for every
/// [`PurposeKindApi`] variant so the panel never renders a blank row.
pub fn purpose_label(purpose: PurposeKindApi) -> &'static str {
    let _ = purpose;
    ""
}

/// One-line description of what a purpose's LLM is used for (panel sub-label).
pub fn purpose_hint(purpose: PurposeKindApi) -> &'static str {
    let _ = purpose;
    ""
}

/// True when a config inherits the interactive purpose — i.e. both `connection`
/// and `model` are the [`PRIMARY`] sentinel.
pub fn is_inherit(cfg: &PurposeConfigView) -> bool {
    let _ = cfg;
    false
}

/// The connection dropdown options as `(value, label)` pairs. Non-interactive
/// purposes get a leading "inherit" option (`value == PRIMARY`); the interactive
/// purpose does not (it cannot inherit).
pub fn connection_options(connections: &[ConnectionView], interactive: bool) -> Vec<(String, String)> {
    let _ = (connections, interactive);
    Vec::new()
}

/// The model dropdown options as `(value, label)` pairs for the currently
/// selected `connection`: the single inherit option when `connection == PRIMARY`,
/// that connection's models when it is a real id, or empty when unset.
pub fn model_options(models: &[ModelListing], connection: &str) -> Vec<(String, String)> {
    let _ = (models, connection);
    Vec::new()
}

/// The model to select when `connection` becomes the active choice: the sentinel
/// for inherit, that connection's first model for a real id, or empty when the
/// connection is unset / has no models.
pub fn reset_model_for_connection(connection: &str, models: &[ModelListing]) -> String {
    let _ = (connection, models);
    String::new()
}

/// Editable state for one purpose slot. Mirrors the TUI's `EditState` and the
/// GTK row: the chosen `connection`/`model` (a real id or the [`PRIMARY`]
/// sentinel), an optional effort override, and the daemon's
/// `max_context_tokens` preserved verbatim.
///
/// `max_context_tokens` is *not* editable here, but `SetPurpose` is a full
/// replace — so we echo back whatever the daemon reported, or a save would wipe
/// an override set elsewhere (TUI/config). Same rationale as the GTK client.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PurposeDraft {
    pub purpose: PurposeKindApi,
    pub connection: String,
    pub model: String,
    pub effort: Option<EffortLevel>,
    pub max_context_tokens: Option<u64>,
}

impl PurposeDraft {
    /// Seed a draft from the daemon's config for `purpose`. An existing config
    /// is copied verbatim; with no config, a non-interactive purpose defaults to
    /// inherit ([`PRIMARY`]/[`PRIMARY`]) and the interactive purpose to blank
    /// (the user must pick a real pair before saving).
    pub fn from_config(purpose: PurposeKindApi, cfg: Option<&PurposeConfigView>) -> Self {
        let _ = cfg;
        Self {
            purpose,
            connection: String::new(),
            model: String::new(),
            effort: None,
            max_context_tokens: None,
        }
    }

    /// True for the interactive purpose, which cannot inherit.
    pub fn is_interactive(&self) -> bool {
        matches!(self.purpose, PurposeKindApi::Interactive)
    }

    /// Change the selected connection, resetting the model to a valid default
    /// for it (see [`reset_model_for_connection`]) so a stale model from the
    /// previous connection can't linger.
    pub fn select_connection(&mut self, connection: String, models: &[ModelListing]) {
        let _ = (connection, models);
    }

    /// Validate the draft and build the [`PurposeConfigView`] to send via
    /// `SetPurpose`. Rejects a blank pick, the interactive purpose trying to
    /// inherit, and a mixed inherit pair (one field `PRIMARY`, the other not).
    pub fn to_config(&self) -> Result<PurposeConfigView, String> {
        unimplemented!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desktop_assistant_api_model::{
        ConnectionAvailability, ModelCapabilitiesView, ModelInfoView,
    };

    fn cfg(connection: &str, model: &str) -> PurposeConfigView {
        PurposeConfigView {
            connection: connection.into(),
            model: model.into(),
            effort: None,
            max_context_tokens: None,
        }
    }

    fn connection(id: &str, connector_type: &str) -> ConnectionView {
        ConnectionView {
            id: id.into(),
            connector_type: connector_type.into(),
            display_label: format!("{id} ({connector_type})"),
            availability: ConnectionAvailability::Ok,
            has_credentials: true,
            config: None,
        }
    }

    fn listing(conn: &str, model: &str) -> ModelListing {
        ModelListing {
            connection_id: conn.into(),
            connection_label: format!("{conn} (test)"),
            model: ModelInfoView {
                id: model.into(),
                display_name: model.into(),
                context_limit: None,
                capabilities: ModelCapabilitiesView::default(),
            },
        }
    }

    // --- slot mapping + six-purpose coverage ---------------------------------

    #[test]
    fn slot_config_maps_each_of_the_six_purposes() {
        // A distinct config per slot; `slot_config` must return the matching one
        // for every `PurposeKindApi`, voice included.
        let view = PurposesView {
            interactive: Some(cfg("i", "mi")),
            dreaming: Some(cfg("d", "md")),
            consolidation: Some(cfg("c", "mc")),
            embedding: Some(cfg("e", "me")),
            titling: Some(cfg("t", "mt")),
            voice: Some(cfg("v", "mv")),
        };
        let expected = [
            (PurposeKindApi::Interactive, "i"),
            (PurposeKindApi::Dreaming, "d"),
            (PurposeKindApi::Consolidation, "c"),
            (PurposeKindApi::Embedding, "e"),
            (PurposeKindApi::Titling, "t"),
            (PurposeKindApi::Voice, "v"),
        ];
        for (purpose, conn) in expected {
            let got = slot_config(&view, purpose).expect("slot present");
            assert_eq!(got.connection, conn, "wrong slot for {purpose:?}");
        }
    }

    #[test]
    fn slot_config_is_none_when_unconfigured() {
        let view = PurposesView::default();
        for purpose in PurposeKindApi::all() {
            assert!(
                slot_config(&view, purpose).is_none(),
                "{purpose:?} should be unconfigured"
            );
        }
    }

    #[test]
    fn all_six_purposes_have_nonempty_labels_including_voice() {
        let all = PurposeKindApi::all();
        assert_eq!(all.len(), 6, "there are six purposes");
        assert!(all.contains(&PurposeKindApi::Voice), "voice must be included");
        for purpose in all {
            assert!(
                !purpose_label(purpose).is_empty(),
                "{purpose:?} needs a label"
            );
            assert!(
                !purpose_hint(purpose).is_empty(),
                "{purpose:?} needs a hint"
            );
        }
    }

    #[test]
    fn is_interactive_only_for_the_interactive_purpose() {
        assert!(PurposeDraft::from_config(PurposeKindApi::Interactive, None).is_interactive());
        for purpose in [
            PurposeKindApi::Dreaming,
            PurposeKindApi::Consolidation,
            PurposeKindApi::Embedding,
            PurposeKindApi::Titling,
            PurposeKindApi::Voice,
        ] {
            assert!(!PurposeDraft::from_config(purpose, None).is_interactive());
        }
    }

    // --- from_config seeding -------------------------------------------------

    #[test]
    fn from_config_uses_existing_values() {
        let existing = PurposeConfigView {
            connection: "work".into(),
            model: "claude".into(),
            effort: Some(EffortLevel::High),
            max_context_tokens: Some(64_000),
        };
        let draft = PurposeDraft::from_config(PurposeKindApi::Interactive, Some(&existing));
        assert_eq!(draft.connection, "work");
        assert_eq!(draft.model, "claude");
        assert_eq!(draft.effort, Some(EffortLevel::High));
        assert_eq!(draft.max_context_tokens, Some(64_000));
    }

    #[test]
    fn from_config_defaults_non_interactive_to_inherit() {
        let draft = PurposeDraft::from_config(PurposeKindApi::Dreaming, None);
        assert_eq!(draft.connection, PRIMARY);
        assert_eq!(draft.model, PRIMARY);
    }

    #[test]
    fn from_config_defaults_voice_to_inherit() {
        // Voice is non-interactive: with no config it inherits by default.
        let draft = PurposeDraft::from_config(PurposeKindApi::Voice, None);
        assert_eq!(draft.connection, PRIMARY);
        assert_eq!(draft.model, PRIMARY);
    }

    #[test]
    fn from_config_defaults_interactive_to_blank() {
        let draft = PurposeDraft::from_config(PurposeKindApi::Interactive, None);
        assert!(draft.connection.is_empty());
        assert!(draft.model.is_empty());
    }

    // --- to_config validation ------------------------------------------------

    #[test]
    fn to_config_rejects_blank_interactive() {
        let draft = PurposeDraft::from_config(PurposeKindApi::Interactive, None);
        assert!(draft.to_config().is_err());
    }

    #[test]
    fn to_config_rejects_primary_for_interactive() {
        let mut draft = PurposeDraft::from_config(PurposeKindApi::Interactive, None);
        draft.connection = PRIMARY.into();
        draft.model = PRIMARY.into();
        let err = draft.to_config().unwrap_err();
        assert!(err.contains("inherit"), "got: {err}");
    }

    #[test]
    fn to_config_rejects_mixed_primary_pair() {
        let mut draft = PurposeDraft::from_config(PurposeKindApi::Dreaming, None);
        draft.connection = "work".into();
        draft.model = PRIMARY.into();
        let err = draft.to_config().unwrap_err();
        assert!(err.to_lowercase().contains("primary"), "got: {err}");
    }

    #[test]
    fn to_config_accepts_inherit_for_non_interactive() {
        let draft = PurposeDraft::from_config(PurposeKindApi::Embedding, None);
        let out = draft.to_config().expect("inherit is valid");
        assert_eq!(out.connection, PRIMARY);
        assert_eq!(out.model, PRIMARY);
        assert!(is_inherit(&out));
    }

    #[test]
    fn to_config_accepts_real_pair_and_preserves_effort() {
        let mut draft = PurposeDraft::from_config(PurposeKindApi::Interactive, None);
        draft.connection = "work".into();
        draft.model = "claude".into();
        draft.effort = Some(EffortLevel::Medium);
        let out = draft.to_config().expect("a real pair is valid");
        assert_eq!(out.connection, "work");
        assert_eq!(out.model, "claude");
        assert_eq!(out.effort, Some(EffortLevel::Medium));
    }

    #[test]
    fn to_config_preserves_max_context_round_trip() {
        let existing = PurposeConfigView {
            connection: "work".into(),
            model: "claude".into(),
            effort: None,
            max_context_tokens: Some(200_000),
        };
        let draft = PurposeDraft::from_config(PurposeKindApi::Titling, Some(&existing));
        let out = draft.to_config().expect("valid");
        assert_eq!(
            out.max_context_tokens,
            Some(200_000),
            "max_context_tokens must survive an edit that never touches it"
        );
    }

    #[test]
    fn to_config_round_trips_every_effort_level() {
        for effort in [
            None,
            Some(EffortLevel::Low),
            Some(EffortLevel::Medium),
            Some(EffortLevel::High),
        ] {
            let mut draft = PurposeDraft::from_config(PurposeKindApi::Dreaming, None);
            draft.connection = "work".into();
            draft.model = "claude".into();
            draft.effort = effort;
            let out = draft.to_config().expect("valid");
            assert_eq!(out.effort, effort);
        }
    }

    // --- is_inherit ----------------------------------------------------------

    #[test]
    fn is_inherit_true_only_when_both_primary() {
        assert!(is_inherit(&cfg(PRIMARY, PRIMARY)));
        assert!(!is_inherit(&cfg("work", PRIMARY)));
        assert!(!is_inherit(&cfg(PRIMARY, "claude")));
        assert!(!is_inherit(&cfg("work", "claude")));
    }

    // --- dropdown option builders --------------------------------------------

    #[test]
    fn connection_options_prepends_inherit_for_non_interactive() {
        let conns = vec![connection("work", "anthropic"), connection("home", "ollama")];
        let non_interactive = connection_options(&conns, false);
        assert_eq!(non_interactive[0].0, PRIMARY, "inherit option comes first");
        assert_eq!(non_interactive[1].0, "work");
        assert_eq!(non_interactive[2].0, "home");

        let interactive = connection_options(&conns, true);
        assert!(
            !interactive.iter().any(|(v, _)| v == PRIMARY),
            "interactive can't inherit, so no primary option"
        );
        assert_eq!(interactive[0].0, "work");
    }

    #[test]
    fn model_options_for_inherit_connection_is_single_primary() {
        let models = vec![listing("work", "claude")];
        let opts = model_options(&models, PRIMARY);
        assert_eq!(opts.len(), 1);
        assert_eq!(opts[0].0, PRIMARY);
    }

    #[test]
    fn model_options_filters_by_connection_without_primary() {
        let models = vec![
            listing("work", "claude"),
            listing("work", "haiku"),
            listing("home", "llama"),
        ];
        let opts = model_options(&models, "work");
        let values: Vec<&str> = opts.iter().map(|(v, _)| v.as_str()).collect();
        assert_eq!(values, vec!["claude", "haiku"]);
        assert!(!values.contains(&PRIMARY), "no inherit option for a real connection");
    }

    #[test]
    fn model_options_empty_when_connection_unset() {
        let models = vec![listing("work", "claude")];
        assert!(model_options(&models, "").is_empty());
    }

    #[test]
    fn reset_model_for_connection_covers_primary_real_and_empty() {
        let models = vec![listing("work", "claude"), listing("work", "haiku")];
        assert_eq!(reset_model_for_connection(PRIMARY, &models), PRIMARY);
        assert_eq!(reset_model_for_connection("work", &models), "claude");
        assert_eq!(reset_model_for_connection("nope", &models), "");
        assert_eq!(reset_model_for_connection("", &models), "");
    }

    #[test]
    fn select_connection_switches_connection_and_resets_model() {
        let models = vec![listing("work", "claude"), listing("home", "llama")];
        let mut draft = PurposeDraft::from_config(PurposeKindApi::Dreaming, None);
        draft.select_connection("work".into(), &models);
        assert_eq!(draft.connection, "work");
        assert_eq!(draft.model, "claude", "model resets to the new connection's first");

        draft.select_connection("home".into(), &models);
        assert_eq!(draft.model, "llama");

        draft.select_connection(PRIMARY.into(), &models);
        assert_eq!(draft.connection, PRIMARY);
        assert_eq!(draft.model, PRIMARY, "inherit resets model to the sentinel");
    }
}
