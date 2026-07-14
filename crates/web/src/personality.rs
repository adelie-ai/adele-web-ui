//! Per-conversation personality override (issue #13): the "Expressive 7" trait
//! dials. Each trait is either **Global** — inherit the daemon's global
//! disposition — or pinned to a concrete [`PersonalityLevel`]
//! (Never/Rarely/Sometimes/Often/Always) for this conversation only.
//!
//! Unlike the header model pill (a live per-send override), the personality
//! override is *persisted* on the daemon: Save issues
//! [`Command::SetConversationPersonality`], the daemon stores the partial
//! override and resolves each `None` trait against the global config on every
//! send. An all-**Global** selection clears the override (back to global-only).
//! The current value is read from [`ConversationView::conversation_personality`]
//! via `GetConversation`, which the panel pre-fills from.
//!
//! **Split like `model.rs`/`purposes.rs`.** The pure trait ⇄
//! [`ConversationPersonalityView`] mapping (the `<select>` value ⇄
//! [`PersonalityLevel`] contract, per-trait get/set, the option list) lives here
//! and unit-tests on the host target; the Leptos panel is a
//! `#[cfg(target_arch = "wasm32")]` submodule below and consumes *these* helpers,
//! so the tested logic and the rendered logic can't drift.
//!
//! [`Command::SetConversationPersonality`]: desktop_assistant_api_model::Command::SetConversationPersonality
//! [`ConversationView::conversation_personality`]: desktop_assistant_api_model::ConversationView

use desktop_assistant_api_model::{ConversationPersonalityView, PersonalityLevel};

/// One of the "Expressive 7" traits, in the wire-contract order — the field
/// order of [`ConversationPersonalityView`] (a `PersonalityOverride`):
/// professionalism, warmth, directness, enthusiasm, humor, sarcasm,
/// pretentiousness.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PersonalityTrait {
    Professionalism,
    Warmth,
    Directness,
    Enthusiasm,
    Humor,
    Sarcasm,
    Pretentiousness,
}

impl PersonalityTrait {
    /// Every trait, in the canonical wire order. The panel renders one row each.
    pub const ALL: [PersonalityTrait; 7] = [
        PersonalityTrait::Professionalism,
        PersonalityTrait::Warmth,
        PersonalityTrait::Directness,
        PersonalityTrait::Enthusiasm,
        PersonalityTrait::Humor,
        PersonalityTrait::Sarcasm,
        PersonalityTrait::Pretentiousness,
    ];

    /// The user-facing row title (title-cased). Defined for every variant so a
    /// row is never blank.
    pub fn label(self) -> &'static str {
        match self {
            PersonalityTrait::Professionalism => "Professionalism",
            PersonalityTrait::Warmth => "Warmth",
            PersonalityTrait::Directness => "Directness",
            PersonalityTrait::Enthusiasm => "Enthusiasm",
            PersonalityTrait::Humor => "Humor",
            PersonalityTrait::Sarcasm => "Sarcasm",
            PersonalityTrait::Pretentiousness => "Pretentiousness",
        }
    }

    /// Read this trait's override value out of `over`. `None` = inherit global.
    pub fn get(self, over: &ConversationPersonalityView) -> Option<PersonalityLevel> {
        match self {
            PersonalityTrait::Professionalism => over.professionalism,
            PersonalityTrait::Warmth => over.warmth,
            PersonalityTrait::Directness => over.directness,
            PersonalityTrait::Enthusiasm => over.enthusiasm,
            PersonalityTrait::Humor => over.humor,
            PersonalityTrait::Sarcasm => over.sarcasm,
            PersonalityTrait::Pretentiousness => over.pretentiousness,
        }
    }

    /// Pin (or clear) this trait's override value in `over`. `None` clears it
    /// back to "inherit global".
    pub fn set(self, over: &mut ConversationPersonalityView, level: Option<PersonalityLevel>) {
        match self {
            PersonalityTrait::Professionalism => over.professionalism = level,
            PersonalityTrait::Warmth => over.warmth = level,
            PersonalityTrait::Directness => over.directness = level,
            PersonalityTrait::Enthusiasm => over.enthusiasm = level,
            PersonalityTrait::Humor => over.humor = level,
            PersonalityTrait::Sarcasm => over.sarcasm = level,
            PersonalityTrait::Pretentiousness => over.pretentiousness = level,
        }
    }
}

/// The five concrete levels, ascending (Never … Always) — the non-Global
/// dropdown options.
pub const LEVELS: [PersonalityLevel; 5] = [
    PersonalityLevel::Never,
    PersonalityLevel::Rarely,
    PersonalityLevel::Sometimes,
    PersonalityLevel::Often,
    PersonalityLevel::Always,
];

/// The `<select>` option `value` for a level: the empty string for `None`
/// ("Global"), else the level's lowercase name (matching the wire's serde
/// `rename_all = "lowercase"`, though these strings are DOM-local, not wire).
pub fn value_from_level(level: Option<PersonalityLevel>) -> &'static str {
    match level {
        None => "",
        Some(PersonalityLevel::Never) => "never",
        Some(PersonalityLevel::Rarely) => "rarely",
        Some(PersonalityLevel::Sometimes) => "sometimes",
        Some(PersonalityLevel::Often) => "often",
        Some(PersonalityLevel::Always) => "always",
    }
}

/// Inverse of [`value_from_level`]: map a `<select>` value back to a level. The
/// empty string, "global", and any unrecognized value all fall back to `None`
/// ("inherit global"), so a malformed selection degrades safely rather than
/// panicking.
pub fn level_from_value(value: &str) -> Option<PersonalityLevel> {
    match value {
        "never" => Some(PersonalityLevel::Never),
        "rarely" => Some(PersonalityLevel::Rarely),
        "sometimes" => Some(PersonalityLevel::Sometimes),
        "often" => Some(PersonalityLevel::Often),
        "always" => Some(PersonalityLevel::Always),
        // "", "global", and anything unrecognized → inherit global.
        _ => None,
    }
}

/// The user-facing label for a concrete level ("Never" … "Always").
pub fn level_label(level: PersonalityLevel) -> &'static str {
    match level {
        PersonalityLevel::Never => "Never",
        PersonalityLevel::Rarely => "Rarely",
        PersonalityLevel::Sometimes => "Sometimes",
        PersonalityLevel::Often => "Often",
        PersonalityLevel::Always => "Always",
    }
}

/// The dropdown options for one trait row as `(value, label)` pairs: the leading
/// "Global (inherit)" sentinel (`value == ""`) followed by the five levels.
pub fn row_options() -> Vec<(&'static str, &'static str)> {
    let mut opts = Vec::with_capacity(1 + LEVELS.len());
    opts.push(("", "Global (inherit)"));
    for level in LEVELS {
        opts.push((value_from_level(Some(level)), level_label(level)));
    }
    opts
}

/// How many of the seven traits are pinned (non-`None`) in `over`. Drives the
/// panel's "N of 7 pinned" summary; `0` means the conversation inherits the
/// global personality wholesale.
pub fn override_count(over: &ConversationPersonalityView) -> usize {
    PersonalityTrait::ALL
        .into_iter()
        .filter(|t| t.get(over).is_some())
        .count()
}

/// The Leptos personality panel (issue #13). Re-exported from the wasm-only
/// [`ui`] submodule; `settings.rs` renders it as the `Personality` panel body.
#[cfg(target_arch = "wasm32")]
pub use ui::personality_panel;

#[cfg(target_arch = "wasm32")]
mod ui {
    //! Mobile-first Leptos view: one stacked card holding a row per trait. Each
    //! row is a native `<select>` (touch-friendly on phones) offering **Global
    //! (inherit)** plus the five levels. A single Save button appears once any
    //! row is edited; setting every row back to Global and saving clears the
    //! override. Save persists via the engine's `set_personality`
    //! (`SetConversationPersonality`); Refresh re-reads the stored override from
    //! `GetConversation`.

    use leptos::prelude::*;

    use desktop_assistant_api_model::ConversationPersonalityView;

    use super::{
        PersonalityTrait, level_from_value, override_count, row_options, value_from_level,
    };
    use crate::engine::ViewSignals;
    use crate::settings::EngineHandle;

    /// The panel body. Loads the conversation's stored override once on first
    /// open (and via the Refresh button), then renders the trait card.
    pub fn personality_panel(engine: EngineHandle, view: ViewSignals) -> impl IntoView {
        // Load once when the panel first opens. `personality_loaded` (a separate
        // flag from `personality`, which is legitimately `None` for "no
        // override") distinguishes "not yet fetched" from "fetched, empty";
        // `refresh_personality` sets `busy` synchronously so a re-render before
        // the fetch resolves can't kick a second load.
        if !view.personality_loaded.get_untracked() && !view.personality_busy.get_untracked() {
            engine.with_value(|e| e.borrow().refresh_personality());
        }
        let refresh = move |_| engine.with_value(|e| e.borrow().refresh_personality());

        view! {
            <section class="panel personality-panel">
                <div class="panel-intro">
                    <p class="panel-summary">{move || summary(view)}</p>
                    <p class="panel-note muted">
                        "Each trait set to Global inherits your global setting; pin a level to \
                         override it for this conversation only. It sets the initial disposition — \
                         Adele still adapts as you talk."
                    </p>
                </div>

                <div class="field-head">
                    <span class="field-label">"Traits"</span>
                    <button class="link" on:click=refresh>
                        {move || if view.personality_busy.get() { "Working…" } else { "Refresh" }}
                    </button>
                </div>

                {move || {
                    if !view.personality_loaded.get() {
                        view! { <p class="empty muted">"Loading personality…"</p> }.into_any()
                    } else {
                        // Read `personality` so a save/refresh re-seeds the form
                        // (dirty → clean) when the stored override changes.
                        let stored = view.personality.get();
                        personality_form(engine, view, stored).into_any()
                    }
                }}
            </section>
        }
    }

    /// A one-line summary of how many traits are pinned for this conversation.
    fn summary(view: ViewSignals) -> String {
        match view.personality.get().as_ref().map(override_count) {
            None | Some(0) => "No overrides — inherits your global personality.".to_string(),
            Some(1) => "1 of 7 traits pinned for this conversation.".to_string(),
            Some(n) => format!("{n} of 7 traits pinned for this conversation."),
        }
    }

    /// The trait card: seven `<select>` rows seeded from `stored`, plus a
    /// dirty-gated Save. Edits stage into a single [`ConversationPersonalityView`]
    /// draft (it is `Copy`); Save persists it and the daemon echo re-seeds.
    fn personality_form(
        engine: EngineHandle,
        view: ViewSignals,
        stored: Option<ConversationPersonalityView>,
    ) -> impl IntoView {
        let saved = stored.unwrap_or_default();
        let draft = RwSignal::new(saved);
        let dirty = Signal::derive(move || draft.get() != saved);
        let save = move |_| {
            let over = draft.get_untracked();
            engine.with_value(|e| e.borrow().set_personality(over));
        };

        view! {
            <div class="purpose-card personality-card">
                {PersonalityTrait::ALL
                    .into_iter()
                    .map(|t| trait_row(draft, t))
                    .collect_view()}

                <Show when=move || dirty.get()>
                    <button
                        class="save-purpose"
                        disabled=move || view.personality_busy.get()
                        on:click=save
                    >
                        "Save"
                    </button>
                </Show>
            </div>
        }
    }

    /// One trait's labelled `<select>`. The current value drives the selected
    /// option reactively; changing it stages the new level into the draft.
    fn trait_row(
        draft: RwSignal<ConversationPersonalityView>,
        t: PersonalityTrait,
    ) -> impl IntoView {
        let on_change = move |ev: leptos::ev::Event| {
            let level = level_from_value(&event_target_value(&ev));
            draft.update(|d| t.set(d, level));
        };
        view! {
            <label class="purpose-field personality-row">
                <span class="sub-label">{t.label()}</span>
                <select class="select" aria-label=t.label() on:change=on_change>
                    {move || {
                        let current = value_from_level(t.get(&draft.get()));
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

    /// A `ConversationPersonalityView` with only the named traits pinned.
    fn over(pairs: &[(PersonalityTrait, PersonalityLevel)]) -> ConversationPersonalityView {
        let mut o = ConversationPersonalityView::default();
        for (t, level) in pairs {
            t.set(&mut o, Some(*level));
        }
        o
    }

    #[test]
    fn all_has_seven_traits_in_canonical_order() {
        // The panel order must match the wire field order exactly.
        assert_eq!(
            PersonalityTrait::ALL,
            [
                PersonalityTrait::Professionalism,
                PersonalityTrait::Warmth,
                PersonalityTrait::Directness,
                PersonalityTrait::Enthusiasm,
                PersonalityTrait::Humor,
                PersonalityTrait::Sarcasm,
                PersonalityTrait::Pretentiousness,
            ]
        );
    }

    #[test]
    fn every_trait_has_a_nonempty_label() {
        for t in PersonalityTrait::ALL {
            assert!(!t.label().is_empty(), "{t:?} needs a label");
        }
    }

    #[test]
    fn get_and_set_target_the_right_field_with_no_cross_talk() {
        // Pin each trait to a *distinct* level, then assert each reads back its
        // own value and nothing bled into a neighbour — this catches a swapped
        // field mapping.
        let assignments = [
            (PersonalityTrait::Professionalism, PersonalityLevel::Never),
            (PersonalityTrait::Warmth, PersonalityLevel::Rarely),
            (PersonalityTrait::Directness, PersonalityLevel::Sometimes),
            (PersonalityTrait::Enthusiasm, PersonalityLevel::Often),
            (PersonalityTrait::Humor, PersonalityLevel::Always),
            (PersonalityTrait::Sarcasm, PersonalityLevel::Never),
            (PersonalityTrait::Pretentiousness, PersonalityLevel::Rarely),
        ];
        let o = over(&assignments);
        for (t, level) in assignments {
            assert_eq!(t.get(&o), Some(level), "{t:?} read back wrong");
        }
        // Spot-check the concrete fields land where the wire expects them.
        assert_eq!(o.professionalism, Some(PersonalityLevel::Never));
        assert_eq!(o.humor, Some(PersonalityLevel::Always));
        assert_eq!(o.pretentiousness, Some(PersonalityLevel::Rarely));
    }

    #[test]
    fn set_none_clears_a_trait() {
        let mut o = over(&[(PersonalityTrait::Humor, PersonalityLevel::Always)]);
        assert_eq!(o.humor, Some(PersonalityLevel::Always));
        PersonalityTrait::Humor.set(&mut o, None);
        assert_eq!(o.humor, None, "None must clear the pin");
        assert!(
            o.is_empty(),
            "clearing the only pin leaves an empty override"
        );
    }

    #[test]
    fn value_and_level_round_trip_for_every_level() {
        for level in LEVELS {
            let value = value_from_level(Some(level));
            assert_eq!(
                level_from_value(value),
                Some(level),
                "round-trip {level:?} via {value:?}"
            );
        }
    }

    #[test]
    fn global_is_the_empty_value_both_ways() {
        assert_eq!(value_from_level(None), "");
        assert_eq!(level_from_value(""), None);
    }

    #[test]
    fn unrecognized_value_falls_back_to_global() {
        // A malformed / stale selection must degrade to "inherit global", never
        // panic or snap to a level.
        assert_eq!(level_from_value("global"), None);
        assert_eq!(level_from_value("NEVER"), None); // case-sensitive: not a match
        assert_eq!(level_from_value("garbage"), None);
    }

    #[test]
    fn level_labels_are_nonempty_and_distinct() {
        let labels: Vec<&str> = LEVELS.iter().map(|l| level_label(*l)).collect();
        for label in &labels {
            assert!(!label.is_empty());
        }
        let mut sorted = labels.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), labels.len(), "level labels must be distinct");
    }

    #[test]
    fn row_options_are_global_plus_five_levels() {
        let opts = row_options();
        assert_eq!(opts.len(), 6, "Global + 5 levels");
        assert_eq!(
            opts[0].0, "",
            "Global sentinel is first with an empty value"
        );
        // Every non-Global option's value maps back to a concrete level.
        for (value, _label) in &opts[1..] {
            assert!(
                level_from_value(value).is_some(),
                "option {value:?} must be a real level"
            );
        }
        // The five level values, in order, are exactly the ascending levels.
        let level_values: Vec<Option<PersonalityLevel>> =
            opts[1..].iter().map(|(v, _)| level_from_value(v)).collect();
        assert_eq!(
            level_values,
            LEVELS.iter().copied().map(Some).collect::<Vec<_>>()
        );
    }

    #[test]
    fn override_count_counts_pinned_traits() {
        assert_eq!(
            override_count(&ConversationPersonalityView::default()),
            0,
            "an empty override pins nothing"
        );
        let partial = over(&[
            (PersonalityTrait::Humor, PersonalityLevel::Never),
            (PersonalityTrait::Directness, PersonalityLevel::Always),
        ]);
        assert_eq!(override_count(&partial), 2);
        // All seven pinned.
        let full = over(&[
            (PersonalityTrait::Professionalism, PersonalityLevel::Always),
            (PersonalityTrait::Warmth, PersonalityLevel::Often),
            (PersonalityTrait::Directness, PersonalityLevel::Often),
            (PersonalityTrait::Enthusiasm, PersonalityLevel::Sometimes),
            (PersonalityTrait::Humor, PersonalityLevel::Sometimes),
            (PersonalityTrait::Sarcasm, PersonalityLevel::Rarely),
            (PersonalityTrait::Pretentiousness, PersonalityLevel::Rarely),
        ]);
        assert_eq!(override_count(&full), 7);
    }

    #[test]
    fn a_partial_override_round_trips_through_get_set() {
        // The issue's acceptance example: humor=Never, directness=Always, the
        // rest inherited. Rebuilding trait-by-trait yields the same override.
        let stored = over(&[
            (PersonalityTrait::Humor, PersonalityLevel::Never),
            (PersonalityTrait::Directness, PersonalityLevel::Always),
        ]);
        let mut rebuilt = ConversationPersonalityView::default();
        for t in PersonalityTrait::ALL {
            rebuilt = {
                let mut r = rebuilt;
                t.set(&mut r, t.get(&stored));
                r
            };
        }
        assert_eq!(rebuilt, stored);
        assert_eq!(override_count(&stored), 2);
    }
}
