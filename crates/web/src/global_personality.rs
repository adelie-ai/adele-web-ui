//! Global personality settings (issue #17): the daemon's **default** "Expressive
//! 7" disposition that every conversation inherits — distinct from the
//! per-conversation override in [`crate::personality`] (#13).
//!
//! Where the per-conversation panel edits a *partial* [`PersonalityOverride`]
//! (each trait `Some(level)` or `None` = inherit global), the global config is a
//! *complete* [`Personality`] — every trait always has a concrete
//! [`PersonalityLevel`], so there is no "Global (inherit)" sentinel here.
//!
//! **Command surface (scoped from `desktop-assistant`).** The global personality
//! lives in the transport-level config API:
//! - [`Command::GetConfig`] → [`CommandResult::Config`]`(Config)`, whose
//!   `personality: PersonalitySettingsView` field carries the 7 trait levels.
//! - [`Command::SetConfig`]`{ changes: ConfigChanges }` → the same
//!   `CommandResult::Config`, echoing the stored config after the write. Each
//!   `personality_*` field of [`ConfigChanges`] is an `Option<PersonalityLevel>`:
//!   `Some` overrides just that trait, `None` leaves it unchanged. We always send
//!   all seven (a full replace), so [`changes_from`] sets every one.
//!
//! Both commands blind-forward through the BFF (no BFF/daemon change). The shared
//! reducer does not model `Config`, so — like the purposes and per-conversation
//! personality panels — the engine writes straight to the view signals.
//!
//! **Split like [`crate::personality`].** The pure trait ⇄ [`Personality`]
//! mapping and the `Personality` → [`ConfigChanges`] wire mapping live here and
//! unit-test on the host target; the Leptos panel is a
//! `#[cfg(target_arch = "wasm32")]` submodule that consumes *these* helpers, so
//! the tested logic and the rendered logic can't drift. It reuses
//! [`crate::personality`]'s [`PersonalityTrait`] enum (canonical trait order +
//! labels) and level helpers rather than redefining them.
//!
//! [`Command::GetConfig`]: desktop_assistant_api_model::Command::GetConfig
//! [`Command::SetConfig`]: desktop_assistant_api_model::Command::SetConfig
//! [`CommandResult::Config`]: desktop_assistant_api_model::CommandResult::Config
//! [`Personality`]: desktop_assistant_api_model::PersonalitySettingsView

use desktop_assistant_api_model::{ConfigChanges, PersonalityLevel, PersonalitySettingsView};

use crate::personality::{LEVELS, PersonalityTrait, value_from_level};

/// Read one trait's level out of the complete global [`PersonalitySettingsView`].
/// Unlike the per-conversation `get` (which returns `Option`), every global trait
/// always has a concrete level.
pub fn get(t: PersonalityTrait, p: &PersonalitySettingsView) -> PersonalityLevel {
    let _ = (t, p);
    unimplemented!("global_personality::get")
}

/// Set one trait's level in the global [`PersonalitySettingsView`].
pub fn set(t: PersonalityTrait, p: &mut PersonalitySettingsView, level: PersonalityLevel) {
    let _ = (t, p, level);
    unimplemented!("global_personality::set")
}

/// The `<select>` options for one trait row: exactly the five concrete levels
/// (Never … Always) as `(value, label)` pairs. There is **no** "Global (inherit)"
/// sentinel — a global trait is always pinned to a real level.
pub fn row_options() -> Vec<(&'static str, &'static str)> {
    unimplemented!("global_personality::row_options")
}

/// Map a complete global [`PersonalitySettingsView`] into a [`ConfigChanges`] that
/// sets **all seven** personality traits (a full replace) and touches nothing
/// else (embeddings / persistence stay `None`). This is the exact wire payload
/// `SetConfig` receives.
pub fn changes_from(p: &PersonalitySettingsView) -> ConfigChanges {
    let _ = p;
    unimplemented!("global_personality::changes_from")
}

/// The Leptos global-personality panel (issue #17). Re-exported from the wasm-only
/// [`ui`] submodule; `settings.rs` renders it as the `GlobalPersonality` panel.
#[cfg(target_arch = "wasm32")]
pub use ui::global_personality_panel;

#[cfg(target_arch = "wasm32")]
mod ui {
    //! Mobile-first Leptos view: one stacked card holding a row per trait. Each
    //! row is a native `<select>` (touch-friendly on phones) offering the five
    //! levels. A single Save button appears once any row is edited and persists
    //! via the engine's `save_global_personality` (`SetConfig`); Refresh re-reads
    //! the stored config (`GetConfig`).

    use leptos::prelude::*;

    use desktop_assistant_api_model::PersonalitySettingsView;

    use super::{PersonalityTrait, get, row_options, set, value_from_level};
    use crate::engine::ViewSignals;
    use crate::personality::level_from_value;
    use crate::settings::EngineHandle;

    /// The panel body. Loads the daemon's global personality once on first open
    /// (and via Refresh), then renders the trait card.
    pub fn global_personality_panel(engine: EngineHandle, view: ViewSignals) -> impl IntoView {
        // Load once when the panel first opens. `global_personality_loaded`
        // distinguishes "not yet fetched" from "fetched"; `refresh_*` sets `busy`
        // synchronously so a re-render before the fetch resolves can't kick a
        // second load.
        if !view.global_personality_loaded.get_untracked()
            && !view.global_personality_busy.get_untracked()
        {
            engine.with_value(|e| e.borrow().refresh_global_personality());
        }
        let refresh = move |_| engine.with_value(|e| e.borrow().refresh_global_personality());

        view! {
            <section class="panel personality-panel global-personality-panel">
                <div class="panel-intro">
                    <p class="panel-summary">
                        "Your global personality — the default disposition every conversation \
                         starts from."
                    </p>
                    <p class="panel-note muted">
                        "These traits set Adele's initial disposition across all conversations. A \
                         conversation's own Personality panel can override any of them for that \
                         chat only. It sets the starting point — Adele still adapts as you talk."
                    </p>
                </div>

                <div class="field-head">
                    <span class="field-label">"Traits"</span>
                    <button class="link" on:click=refresh>
                        {move || {
                            if view.global_personality_busy.get() { "Working…" } else { "Refresh" }
                        }}
                    </button>
                </div>

                {move || {
                    if !view.global_personality_loaded.get() {
                        view! { <p class="empty muted">"Loading personality…"</p> }.into_any()
                    } else {
                        // Read `global_personality` so a save/refresh re-seeds the
                        // form (dirty → clean) when the stored config changes.
                        let stored = view.global_personality.get().unwrap_or_default();
                        global_form(engine, view, stored).into_any()
                    }
                }}
            </section>
        }
    }

    /// The trait card: seven `<select>` rows seeded from `stored`, plus a
    /// dirty-gated Save. Edits stage into a single [`PersonalitySettingsView`]
    /// draft (it is `Copy`); Save persists it and the daemon echo re-seeds.
    fn global_form(
        engine: EngineHandle,
        view: ViewSignals,
        stored: PersonalitySettingsView,
    ) -> impl IntoView {
        let draft = RwSignal::new(stored);
        let dirty = Signal::derive(move || draft.get() != stored);
        let save = move |_| {
            let p = draft.get_untracked();
            engine.with_value(|e| e.borrow().save_global_personality(p));
        };

        view! {
            <div class="purpose-card personality-card global-personality-card">
                {PersonalityTrait::ALL
                    .into_iter()
                    .map(|t| trait_row(draft, t))
                    .collect_view()}

                <Show when=move || dirty.get()>
                    <button
                        class="save-purpose"
                        disabled=move || view.global_personality_busy.get()
                        on:click=save
                    >
                        "Save"
                    </button>
                </Show>
            </div>
        }
    }

    /// One trait's labelled `<select>`. The current value drives the selected
    /// option reactively; changing it stages the new level into the draft. An
    /// unrecognized value (stale option) is ignored, leaving the draft unchanged.
    fn trait_row(draft: RwSignal<PersonalitySettingsView>, t: PersonalityTrait) -> impl IntoView {
        let on_change = move |ev: leptos::ev::Event| {
            if let Some(level) = level_from_value(&event_target_value(&ev)) {
                draft.update(|d| set(t, d, level));
            }
        };
        view! {
            <label class="purpose-field personality-row">
                <span class="sub-label">{t.label()}</span>
                <select class="select" aria-label=t.label() on:change=on_change>
                    {move || {
                        let current = value_from_level(Some(get(t, &draft.get())));
                        row_options()
                            .into_iter()
                            .map(|(value, label)| {
                                view! {
                                    <option value=value selected=value == current>
                                        {label}
                                    </option>
                                }
                            })
                            .collect_view()
                    }}
                </select>
            </label>
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a global personality with each named trait at a given level, the
    /// rest at their Expressive-7 defaults.
    fn personality(pairs: &[(PersonalityTrait, PersonalityLevel)]) -> PersonalitySettingsView {
        let mut p = PersonalitySettingsView::default();
        for (t, level) in pairs {
            set(*t, &mut p, *level);
        }
        p
    }

    #[test]
    fn get_and_set_target_the_right_field_with_no_cross_talk() {
        // Pin each trait to a *distinct* level, then assert each reads back its
        // own value — catches a swapped field mapping.
        let assignments = [
            (PersonalityTrait::Professionalism, PersonalityLevel::Never),
            (PersonalityTrait::Warmth, PersonalityLevel::Rarely),
            (PersonalityTrait::Directness, PersonalityLevel::Sometimes),
            (PersonalityTrait::Enthusiasm, PersonalityLevel::Often),
            (PersonalityTrait::Humor, PersonalityLevel::Always),
            (PersonalityTrait::Sarcasm, PersonalityLevel::Never),
            (PersonalityTrait::Pretentiousness, PersonalityLevel::Rarely),
        ];
        let p = personality(&assignments);
        for (t, level) in assignments {
            assert_eq!(get(t, &p), level, "{t:?} read back wrong");
        }
        // Spot-check the concrete fields land where the wire expects them.
        assert_eq!(p.professionalism, PersonalityLevel::Never);
        assert_eq!(p.humor, PersonalityLevel::Always);
        assert_eq!(p.pretentiousness, PersonalityLevel::Rarely);
    }

    #[test]
    fn set_overwrites_a_previous_value() {
        let mut p = PersonalitySettingsView::default();
        set(PersonalityTrait::Humor, &mut p, PersonalityLevel::Always);
        assert_eq!(get(PersonalityTrait::Humor, &p), PersonalityLevel::Always);
        set(PersonalityTrait::Humor, &mut p, PersonalityLevel::Never);
        assert_eq!(
            get(PersonalityTrait::Humor, &p),
            PersonalityLevel::Never,
            "the second set must replace the first"
        );
    }

    #[test]
    fn get_reads_the_expressive_7_defaults() {
        // Every trait of a default config resolves to a concrete level (no
        // Option), and matches the daemon's Expressive-7 table.
        let p = PersonalitySettingsView::default();
        assert_eq!(
            get(PersonalityTrait::Professionalism, &p),
            PersonalityLevel::Always
        );
        assert_eq!(get(PersonalityTrait::Warmth, &p), PersonalityLevel::Often);
        assert_eq!(
            get(PersonalityTrait::Directness, &p),
            PersonalityLevel::Often
        );
        assert_eq!(
            get(PersonalityTrait::Enthusiasm, &p),
            PersonalityLevel::Sometimes
        );
        assert_eq!(get(PersonalityTrait::Humor, &p), PersonalityLevel::Sometimes);
        assert_eq!(get(PersonalityTrait::Sarcasm, &p), PersonalityLevel::Rarely);
        assert_eq!(
            get(PersonalityTrait::Pretentiousness, &p),
            PersonalityLevel::Rarely
        );
    }

    #[test]
    fn row_options_are_exactly_the_five_levels_no_global_sentinel() {
        let opts = row_options();
        assert_eq!(opts.len(), 5, "five concrete levels, no Global sentinel");
        // No empty-value ("Global (inherit)") option — unlike the per-conversation
        // panel, a global trait is always pinned.
        assert!(
            opts.iter().all(|(value, _)| !value.is_empty()),
            "no empty-value sentinel among {opts:?}"
        );
        // The five values, in order, are exactly the ascending levels.
        let values: Vec<&str> = opts.iter().map(|(v, _)| *v).collect();
        let expected: Vec<&str> = LEVELS.iter().map(|l| value_from_level(Some(*l))).collect();
        assert_eq!(values, expected);
    }

    #[test]
    fn row_option_values_round_trip_back_to_their_level() {
        use crate::personality::level_from_value;
        for (value, _label) in row_options() {
            assert!(
                level_from_value(value).is_some(),
                "option value {value:?} must map back to a concrete level"
            );
        }
    }

    #[test]
    fn changes_from_sets_all_seven_personality_fields() {
        // A fully-specified config maps every trait onto its `personality_*`
        // change, each `Some`, with no cross-talk.
        let p = personality(&[
            (PersonalityTrait::Professionalism, PersonalityLevel::Never),
            (PersonalityTrait::Warmth, PersonalityLevel::Rarely),
            (PersonalityTrait::Directness, PersonalityLevel::Sometimes),
            (PersonalityTrait::Enthusiasm, PersonalityLevel::Often),
            (PersonalityTrait::Humor, PersonalityLevel::Always),
            (PersonalityTrait::Sarcasm, PersonalityLevel::Never),
            (PersonalityTrait::Pretentiousness, PersonalityLevel::Rarely),
        ]);
        let c = changes_from(&p);
        assert_eq!(c.personality_professionalism, Some(PersonalityLevel::Never));
        assert_eq!(c.personality_warmth, Some(PersonalityLevel::Rarely));
        assert_eq!(c.personality_directness, Some(PersonalityLevel::Sometimes));
        assert_eq!(c.personality_enthusiasm, Some(PersonalityLevel::Often));
        assert_eq!(c.personality_humor, Some(PersonalityLevel::Always));
        assert_eq!(c.personality_sarcasm, Some(PersonalityLevel::Never));
        assert_eq!(
            c.personality_pretentiousness,
            Some(PersonalityLevel::Rarely)
        );
    }

    #[test]
    fn changes_from_touches_only_personality_not_embeddings_or_persistence() {
        // A personality save must not disturb unrelated config (embeddings /
        // persistence) — those stay `None` so `SetConfig` leaves them unchanged.
        let c = changes_from(&PersonalitySettingsView::default());
        assert!(c.embeddings_connector.is_none());
        assert!(c.embeddings_model.is_none());
        assert!(c.embeddings_base_url.is_none());
        assert!(c.persistence_enabled.is_none());
        assert!(c.persistence_remote_url.is_none());
        assert!(c.persistence_remote_name.is_none());
        assert!(c.persistence_push_on_update.is_none());
    }

    #[test]
    fn changes_from_serializes_only_personality_fields() {
        // On the wire, a personality save carries exactly the seven
        // `personality_*` keys (skip_serializing_if drops the `None` config
        // fields), so `SetConfig` is an isolated personality write.
        let c = changes_from(&PersonalitySettingsView::default());
        let json = serde_json::to_string(&c).expect("serializes");
        assert!(json.contains("personality_professionalism"), "json: {json}");
        assert!(json.contains("personality_pretentiousness"), "json: {json}");
        assert!(!json.contains("embeddings"), "json: {json}");
        assert!(!json.contains("persistence"), "json: {json}");
    }

    #[test]
    fn a_full_config_round_trips_through_get_and_changes_from() {
        // Rebuilding a config trait-by-trait via get, then mapping to changes and
        // reading each `Some` back, reproduces the original levels for all seven.
        let stored = personality(&[
            (PersonalityTrait::Professionalism, PersonalityLevel::Sometimes),
            (PersonalityTrait::Humor, PersonalityLevel::Never),
            (PersonalityTrait::Directness, PersonalityLevel::Always),
        ]);
        let changes = changes_from(&stored);
        let field = |t: PersonalityTrait| -> Option<PersonalityLevel> {
            match t {
                PersonalityTrait::Professionalism => changes.personality_professionalism,
                PersonalityTrait::Warmth => changes.personality_warmth,
                PersonalityTrait::Directness => changes.personality_directness,
                PersonalityTrait::Enthusiasm => changes.personality_enthusiasm,
                PersonalityTrait::Humor => changes.personality_humor,
                PersonalityTrait::Sarcasm => changes.personality_sarcasm,
                PersonalityTrait::Pretentiousness => changes.personality_pretentiousness,
            }
        };
        for t in PersonalityTrait::ALL {
            assert_eq!(field(t), Some(get(t, &stored)), "{t:?} lost in the mapping");
        }
    }
}
