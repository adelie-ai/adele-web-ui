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
    match purpose {
        PurposeKindApi::Interactive => view.interactive.as_ref(),
        PurposeKindApi::Dreaming => view.dreaming.as_ref(),
        PurposeKindApi::Consolidation => view.consolidation.as_ref(),
        PurposeKindApi::Embedding => view.embedding.as_ref(),
        PurposeKindApi::Titling => view.titling.as_ref(),
        PurposeKindApi::Voice => view.voice.as_ref(),
    }
}

/// Human-readable name for a purpose (title-cased). Defined for every
/// [`PurposeKindApi`] variant so the panel never renders a blank row.
pub fn purpose_label(purpose: PurposeKindApi) -> &'static str {
    match purpose {
        PurposeKindApi::Interactive => "Interactive",
        PurposeKindApi::Dreaming => "Dreaming",
        PurposeKindApi::Consolidation => "Consolidation",
        PurposeKindApi::Embedding => "Embedding",
        PurposeKindApi::Titling => "Titling",
        PurposeKindApi::Voice => "Voice",
    }
}

/// One-line description of what a purpose's LLM is used for (panel sub-label).
pub fn purpose_hint(purpose: PurposeKindApi) -> &'static str {
    match purpose {
        PurposeKindApi::Interactive => "The chat model you talk to.",
        PurposeKindApi::Dreaming => "Periodic memory extraction.",
        PurposeKindApi::Consolidation => "The heavier daily knowledge-base pass.",
        PurposeKindApi::Embedding => "Vector embeddings for memory & search.",
        PurposeKindApi::Titling => "Short conversation titles.",
        PurposeKindApi::Voice => "The voice assistant's turns.",
    }
}

/// True when a config inherits the interactive purpose — i.e. both `connection`
/// and `model` are the [`PRIMARY`] sentinel.
pub fn is_inherit(cfg: &PurposeConfigView) -> bool {
    cfg.connection == PRIMARY && cfg.model == PRIMARY
}

/// The connection dropdown options as `(value, label)` pairs. Non-interactive
/// purposes get a leading "inherit" option (`value == PRIMARY`); the interactive
/// purpose does not (it cannot inherit).
pub fn connection_options(
    connections: &[ConnectionView],
    interactive: bool,
) -> Vec<(String, String)> {
    let mut options = Vec::with_capacity(connections.len() + 1);
    if !interactive {
        options.push((PRIMARY.to_string(), "Inherit from Interactive".to_string()));
    }
    for conn in connections {
        options.push((conn.id.clone(), connection_label(conn)));
    }
    options
}

/// The model dropdown options as `(value, label)` pairs for the currently
/// selected `connection`: the single inherit option when `connection == PRIMARY`,
/// that connection's models when it is a real id, or empty when unset.
pub fn model_options(models: &[ModelListing], connection: &str) -> Vec<(String, String)> {
    if connection == PRIMARY {
        return vec![(PRIMARY.to_string(), "Inherit from Interactive".to_string())];
    }
    if connection.is_empty() {
        return Vec::new();
    }
    models
        .iter()
        .filter(|m| m.connection_id == connection)
        .map(|m| (m.model.id.clone(), model_label(m)))
        .collect()
}

/// The model to select when `connection` becomes the active choice: the sentinel
/// for inherit, that connection's first model for a real id, or empty when the
/// connection is unset / has no models.
pub fn reset_model_for_connection(connection: &str, models: &[ModelListing]) -> String {
    if connection == PRIMARY {
        return PRIMARY.to_string();
    }
    if connection.is_empty() {
        return String::new();
    }
    models
        .iter()
        .find(|m| m.connection_id == connection)
        .map(|m| m.model.id.clone())
        .unwrap_or_default()
}

/// Display label for a connection: its `display_label`, or a synthesized
/// `id (connector_type)` if the daemon left it blank.
fn connection_label(conn: &ConnectionView) -> String {
    if conn.display_label.is_empty() {
        format!("{} ({})", conn.id, conn.connector_type)
    } else {
        conn.display_label.clone()
    }
}

/// Display label for a model row: its `display_name`, or its id when blank.
fn model_label(listing: &ModelListing) -> String {
    if listing.model.display_name.is_empty() {
        listing.model.id.clone()
    } else {
        listing.model.display_name.clone()
    }
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
        match cfg {
            Some(cfg) => Self {
                purpose,
                connection: cfg.connection.clone(),
                model: cfg.model.clone(),
                effort: cfg.effort,
                max_context_tokens: cfg.max_context_tokens,
            },
            // No stored config: non-interactive purposes inherit by default;
            // the interactive purpose has nothing to inherit, so it starts blank
            // and the user must pick a real pair before saving.
            None => {
                let sentinel = if matches!(purpose, PurposeKindApi::Interactive) {
                    String::new()
                } else {
                    PRIMARY.to_string()
                };
                Self {
                    purpose,
                    connection: sentinel.clone(),
                    model: sentinel,
                    effort: None,
                    max_context_tokens: None,
                }
            }
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
        self.model = reset_model_for_connection(&connection, models);
        self.connection = connection;
    }

    /// Validate the draft and build the [`PurposeConfigView`] to send via
    /// `SetPurpose`. Rejects a blank pick, the interactive purpose trying to
    /// inherit, and a mixed inherit pair (one field `PRIMARY`, the other not).
    pub fn to_config(&self) -> Result<PurposeConfigView, String> {
        let connection = self.connection.trim();
        let model = self.model.trim();
        if connection.is_empty() {
            return Err("Pick a connection".into());
        }
        if model.is_empty() {
            return Err("Pick a model".into());
        }
        if self.is_interactive() && (connection == PRIMARY || model == PRIMARY) {
            return Err(
                "The interactive purpose can't inherit — pick a real connection and model.".into(),
            );
        }
        // The daemon's contract: connection and model are *both* "primary"
        // (inherit) or both real ids — never mixed.
        if (connection == PRIMARY) != (model == PRIMARY) {
            return Err("Connection and model must both inherit, or both be a real choice.".into());
        }
        Ok(PurposeConfigView {
            connection: connection.to_string(),
            model: model.to_string(),
            effort: self.effort,
            max_context_tokens: self.max_context_tokens,
        })
    }
}

/// The Leptos Purposes panel (issue #11). Re-exported from the wasm-only [`ui`]
/// submodule; `settings.rs` renders it as the `Purposes` panel body.
#[cfg(target_arch = "wasm32")]
pub use ui::purposes_panel;

#[cfg(target_arch = "wasm32")]
mod ui {
    //! Mobile-first Leptos view: one stacked card per purpose, each with a
    //! connection/model `<select>` (native pickers — touch-friendly), a
    //! segmented effort control, and a Save button that appears once the card is
    //! edited. All six purposes render, `voice` included.

    use leptos::prelude::*;

    use desktop_assistant_api_model::{EffortLevel, PurposeKindApi, PurposesView};

    use super::{
        PurposeDraft, connection_options, is_inherit, model_options, purpose_hint, purpose_label,
        slot_config,
    };
    use crate::engine::ViewSignals;
    use crate::settings::EngineHandle;

    /// The panel body. Loads its data once on first open (and via the Refresh
    /// button), then renders a card per purpose.
    pub fn purposes_panel(engine: EngineHandle, view: ViewSignals) -> impl IntoView {
        // Load once when the panel first opens. `get_untracked` keeps this off
        // the reactive graph; `refresh_purposes` sets `busy` synchronously so a
        // re-render before the fetch resolves can't kick a second load.
        if view.purposes.get_untracked().is_none() && !view.purposes_busy.get_untracked() {
            engine.with_value(|e| e.borrow().refresh_purposes());
        }
        let refresh = move |_| engine.with_value(|e| e.borrow().refresh_purposes());

        view! {
            <section class="panel purposes-panel">
                <div class="panel-intro">
                    <p class="panel-summary">
                        "Route each purpose to a connection and model."
                    </p>
                    <p class="panel-note muted">
                        "Non-interactive purposes can inherit the Interactive choice."
                    </p>
                </div>

                <div class="field-head">
                    <span class="field-label">"Purposes"</span>
                    <button class="link" on:click=refresh>
                        {move || if view.purposes_busy.get() { "Working…" } else { "Refresh" }}
                    </button>
                </div>

                {move || {
                    match view.purposes.get() {
                        None => {
                            view! {
                                <p class="empty muted">"Loading purposes…"</p>
                            }
                                .into_any()
                        }
                        Some(purposes) => {
                            PurposeKindApi::all()
                                .into_iter()
                                .map(|purpose| purpose_card(engine, view, purpose, &purposes))
                                .collect_view()
                                .into_any()
                        }
                    }
                }}
            </section>
        }
    }

    /// One purpose's editable card. Seeds a [`PurposeDraft`] from the loaded
    /// config; edits stage into the draft; Save validates and calls
    /// `SetPurpose`, which re-fetches and re-seeds the card (dirty → clean).
    fn purpose_card(
        engine: EngineHandle,
        view: ViewSignals,
        purpose: PurposeKindApi,
        purposes: &PurposesView,
    ) -> impl IntoView {
        let interactive = matches!(purpose, PurposeKindApi::Interactive);
        let initial = PurposeDraft::from_config(purpose, slot_config(purposes, purpose));
        let saved = initial.clone();
        let draft = RwSignal::new(initial);
        let error = RwSignal::new(Option::<String>::None);
        let dirty = Signal::derive(move || draft.get() != saved);

        let on_connection = move |ev: leptos::ev::Event| {
            let value = event_target_value(&ev);
            let models = view.purpose_models.get_untracked();
            draft.update(|d| d.select_connection(value, &models));
            error.set(None);
        };
        let on_model = move |ev: leptos::ev::Event| {
            let value = event_target_value(&ev);
            draft.update(|d| d.model = value);
            error.set(None);
        };
        let save = move |_| match draft.get_untracked().to_config() {
            Ok(config) => {
                error.set(None);
                engine.with_value(|e| e.borrow().set_purpose(purpose, config));
            }
            Err(e) => error.set(Some(e)),
        };

        view! {
            <div class="purpose-card">
                <div class="purpose-head">
                    <span class="purpose-name">{purpose_label(purpose)}</span>
                    <Show when=move || {
                        draft.get().to_config().map(|c| is_inherit(&c)).unwrap_or(false)
                    }>
                        <span class="purpose-badge">"inherits"</span>
                    </Show>
                </div>
                <p class="purpose-hint muted">{purpose_hint(purpose)}</p>

                <label class="purpose-field">
                    <span class="sub-label">"Connection"</span>
                    <select class="select" on:change=on_connection>
                        {move || {
                            let d = draft.get();
                            let conns = view.purpose_connections.get();
                            let mut opts = connection_options(&conns, interactive);
                            if d.connection.is_empty() {
                                opts.insert(
                                    0,
                                    (String::new(), "Choose a connection…".to_string()),
                                );
                            }
                            opts.into_iter()
                                .map(|(value, label)| {
                                    let selected = value == d.connection;
                                    view! {
                                        <option value=value selected=selected>
                                            {label}
                                        </option>
                                    }
                                })
                                .collect_view()
                        }}
                    </select>
                </label>

                <label class="purpose-field">
                    <span class="sub-label">"Model"</span>
                    <select class="select" on:change=on_model>
                        {move || {
                            let d = draft.get();
                            let models = view.purpose_models.get();
                            let mut opts = model_options(&models, &d.connection);
                            if d.model.is_empty() {
                                opts.insert(0, (String::new(), "Choose a model…".to_string()));
                            }
                            opts.into_iter()
                                .map(|(value, label)| {
                                    let selected = value == d.model;
                                    view! {
                                        <option value=value selected=selected>
                                            {label}
                                        </option>
                                    }
                                })
                                .collect_view()
                        }}
                    </select>
                </label>

                <div class="purpose-field">
                    <span class="sub-label">"Effort"</span>
                    <EffortSegments draft=draft />
                </div>

                <Show when=move || error.get().is_some()>
                    <p class="error">{move || error.get().unwrap_or_default()}</p>
                </Show>

                <Show when=move || dirty.get()>
                    <button
                        class="save-purpose"
                        disabled=move || view.purposes_busy.get()
                        on:click=save
                    >
                        "Save"
                    </button>
                </Show>
            </div>
        }
    }

    /// Auto / Low / Medium / High effort as a segmented control. "Auto" clears
    /// the override so the daemon uses its per-purpose default.
    #[component]
    fn EffortSegments(draft: RwSignal<PurposeDraft>) -> impl IntoView {
        let options: [(&'static str, Option<EffortLevel>); 4] = [
            ("Auto", None),
            ("Low", Some(EffortLevel::Low)),
            ("Medium", Some(EffortLevel::Medium)),
            ("High", Some(EffortLevel::High)),
        ];
        view! {
            <div class="segmented" role="group" aria-label="Effort">
                {options
                    .into_iter()
                    .map(|(label, value)| {
                        let is_active = move || draft.get().effort == value;
                        view! {
                            <button
                                class="segment"
                                class:active=is_active
                                aria-pressed=move || if is_active() { "true" } else { "false" }
                                on:click=move |_| draft.update(|d| d.effort = value)
                            >
                                {label}
                            </button>
                        }
                    })
                    .collect_view()}
            </div>
        }
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
        assert!(
            all.contains(&PurposeKindApi::Voice),
            "voice must be included"
        );
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
        // The mixed-pair error is user-facing, so it speaks of "inherit" rather
        // than leaking the raw "primary" sentinel.
        let err = draft.to_config().unwrap_err();
        assert!(err.to_lowercase().contains("inherit"), "got: {err}");
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
        let conns = vec![
            connection("work", "anthropic"),
            connection("home", "ollama"),
        ];
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
        assert!(
            !values.contains(&PRIMARY),
            "no inherit option for a real connection"
        );
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
        assert_eq!(
            draft.model, "claude",
            "model resets to the new connection's first"
        );

        draft.select_connection("home".into(), &models);
        assert_eq!(draft.model, "llama");

        draft.select_connection(PRIMARY.into(), &models);
        assert_eq!(draft.connection, PRIMARY);
        assert_eq!(draft.model, PRIMARY, "inherit resets model to the sentinel");
    }
}
