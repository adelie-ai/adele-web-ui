//! The settings surface: a mobile-first bottom-sheet drawer that hosts feature
//! panels (issue #8), plus the per-conversation model panel (issue #9).
//!
//! **Mobile-first by design.** The chief use of this client is a phone over
//! Tailscale, so the drawer is a bottom sheet (full-width, slides up, rounded
//! top, `max-height` capped, its own body scroll) rather than a desktop side
//! panel; every tappable target clears 44px; nothing is hover-only. On a wider
//! viewport the same sheet centres as a card — one component, both form factors.
//!
//! **Extensible host.** Panels are a [`SettingsPanel`] enum whose nav is derived
//! from [`SettingsPanel::ALL`]. Adding a panel (connectors, purposes, …) is a
//! localized change: add a variant, its `title`/`icon`, and a body arm in
//! [`panel_body`]. The model panel is the only one wired today.
//!
//! **`!Send` engine, Leptos reactivity.** Leptos 0.8 requires reactive closures
//! and dynamic children to be `Send`, but the [`Engine`] is `Rc`-owned and
//! `!Send`. We hand it around as a [`EngineHandle`] — a `StoredValue` in
//! *local* storage, which is a `Copy`, `Send` handle to a `!Send` value (the
//! standard CSR escape hatch). Reads/writes go through `with_value`.

use std::cell::RefCell;
use std::rc::Rc;

use desktop_assistant_api_model::{EffortLevel, ModelListing};
use leptos::prelude::*;

use client_ui_common::SelectedModel;

use crate::auth;
use crate::engine::{Engine, ViewSignals};
use crate::model;

/// A `Copy`, `Send` handle to the `!Send` [`Engine`], usable from reactive
/// closures and event handlers alike. Call [`with_value`](StoredValue::with_value).
pub type EngineHandle = StoredValue<Rc<RefCell<Engine>>, LocalStorage>;

/// A settings panel the drawer can host. The nav is derived from
/// [`Self::ALL`], so a new panel is one variant + one `panel_body` arm. The
/// [`Default`] is the panel the gear button opens (the first in nav order).
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsPanel {
    #[default]
    Model,
    Connections,
}

impl SettingsPanel {
    /// Every panel, in nav order. Grows as parity panels land.
    pub const ALL: &'static [SettingsPanel] = &[SettingsPanel::Model, SettingsPanel::Connections];

    fn title(self) -> &'static str {
        match self {
            SettingsPanel::Model => "Model",
            SettingsPanel::Connections => "Connections",
        }
    }

    /// A leading glyph for the nav tab (decorative; the title carries meaning).
    fn icon(self) -> &'static str {
        match self {
            SettingsPanel::Model => "\u{1f9e0}",       // brain
            SettingsPanel::Connections => "\u{1f50c}", // electric plug
        }
    }
}

/// The settings drawer. Rendered whenever `open` is `Some(panel)`; that panel is
/// the initially-selected tab. Closing sets `open` to `None`.
#[component]
pub fn SettingsSheet(
    engine: EngineHandle,
    view: ViewSignals,
    /// `None` = closed; `Some(panel)` = open, focused on `panel`.
    open: RwSignal<Option<SettingsPanel>>,
    /// Cleared on sign-out (returns the app to the login screen).
    session: RwSignal<Option<String>>,
) -> impl IntoView {
    let close = move |_| open.set(None);

    let sign_out = move |_| {
        auth::clear_token();
        open.set(None);
        session.set(None);
    };

    view! {
        <Show when=move || open.get().is_some()>
            // Tapping the dim backdrop dismisses; taps inside the sheet don't
            // bubble up to it (`stop_propagation`).
            <div class="settings-backdrop" on:click=close>
                <div
                    class="settings-sheet"
                    role="dialog"
                    aria-modal="true"
                    on:click=|ev| ev.stop_propagation()
                >
                    <div class="sheet-grabber"></div>
                    <header class="sheet-header">
                        <h2>"Settings"</h2>
                        <button class="icon-btn" aria-label="Close settings" on:click=close>
                            "\u{2715}"
                        </button>
                    </header>

                    <nav class="sheet-tabs" aria-label="Settings sections">
                        {SettingsPanel::ALL
                            .iter()
                            .copied()
                            .map(|panel| {
                                view! {
                                    <button
                                        class="sheet-tab"
                                        class:active=move || open.get() == Some(panel)
                                        on:click=move |_| open.set(Some(panel))
                                    >
                                        <span class="tab-icon">{panel.icon()}</span>
                                        <span>{panel.title()}</span>
                                    </button>
                                }
                            })
                            .collect_view()}
                    </nav>

                    <div class="sheet-body">
                        {move || panel_body(open.get().unwrap_or_default(), engine, view)}
                    </div>

                    <footer class="sheet-footer">
                        <button class="link danger" on:click=sign_out>
                            "Sign out"
                        </button>
                    </footer>
                </div>
            </div>
        </Show>
    }
}

/// Render the body for the selected panel. One arm per [`SettingsPanel`].
fn panel_body(panel: SettingsPanel, engine: EngineHandle, view: ViewSignals) -> AnyView {
    match panel {
        SettingsPanel::Model => model_panel(engine, view).into_any(),
        SettingsPanel::Connections => {
            crate::connections::connections_panel(engine, view).into_any()
        }
    }
}

/// The per-conversation model panel (issue #9): current selection, an effort
/// selector, and the available models grouped by connection.
fn model_panel(engine: EngineHandle, view: ViewSignals) -> impl IntoView {
    let refresh = move |_| engine.with_value(|e| e.borrow().refresh_models());

    view! {
        <section class="panel model-panel">
            <div class="panel-intro">
                <p class="panel-summary">{move || current_selection_summary(view)}</p>
                <p class="panel-note muted">
                    "Applies to your next message and is remembered for this conversation."
                </p>
            </div>

            <div class="field">
                <span class="field-label">"Effort"</span>
                <EffortSelector engine=engine view=view />
            </div>

            <div class="field">
                <div class="field-head">
                    <span class="field-label">"Model"</span>
                    <button class="link" on:click=refresh>"Refresh"</button>
                </div>
                {move || {
                    let models = view.models.get();
                    if models.is_empty() {
                        view! {
                            <p class="empty muted">
                                "No models available. Add a connection to choose a model."
                            </p>
                        }
                            .into_any()
                    } else {
                        model_list(engine, view, models).into_any()
                    }
                }}
            </div>
        </section>
    }
}

/// The grouped, tappable model list.
fn model_list(engine: EngineHandle, view: ViewSignals, models: Vec<ModelListing>) -> impl IntoView {
    group_by_connection(&models)
        .into_iter()
        .map(|(conn_id, conn_label, group)| {
            let rows = group
                .into_iter()
                .map(move |listing| model_row(engine, view, conn_id.clone(), listing))
                .collect_view();
            view! {
                <div class="model-group">
                    <h3 class="group-header">{conn_label}</h3>
                    {rows}
                </div>
            }
        })
        .collect_view()
}

/// A single tappable model row: name, context window, capability badges, and a
/// check when it is the active selection.
fn model_row(
    engine: EngineHandle,
    view: ViewSignals,
    connection_id: String,
    listing: ModelListing,
) -> impl IntoView {
    let selection = SelectedModel {
        connection_id,
        model_id: listing.model.id.clone(),
    };
    // `Signal<bool>` is `Copy`, so the same reactive check drives the class, the
    // ARIA state, and the check glyph without juggling closure clones.
    let is_active = {
        let selection = selection.clone();
        Signal::derive(move || view.active_model.get().as_ref() == Some(&selection))
    };
    let pick = move |_| {
        let selection = selection.clone();
        engine.with_value(|e| e.borrow().set_active_model(selection));
    };

    let name = if listing.model.display_name.is_empty() {
        listing.model.id.clone()
    } else {
        listing.model.display_name.clone()
    };
    let caps = listing.model.capabilities;
    let ctx = listing
        .model
        .context_limit
        .map(|n| format!("{} ctx", model::format_context_limit(n)));

    view! {
        <button
            class="model-row"
            class:selected=move || is_active.get()
            aria-pressed=move || if is_active.get() { "true" } else { "false" }
            on:click=pick
        >
            <span class="check" aria-hidden="true">
                {move || if is_active.get() { "\u{2713}" } else { "" }}
            </span>
            <span class="model-main">
                <span class="model-name">{name}</span>
                <span class="badges">
                    {ctx.map(|c| view! { <span class="badge ctx">{c}</span> })}
                    {caps.reasoning.then(|| view! { <span class="badge cap">"reasoning"</span> })}
                    {caps.vision.then(|| view! { <span class="badge cap">"vision"</span> })}
                    {caps.tools.then(|| view! { <span class="badge cap">"tools"</span> })}
                </span>
            </span>
        </button>
    }
}

/// Low / Medium / High effort, plus an "Auto" segment that clears the override
/// so the daemon uses its per-purpose default. A segmented control (touch-sized).
#[component]
fn EffortSelector(engine: EngineHandle, view: ViewSignals) -> impl IntoView {
    // (label, value). `None` = Auto (defer to the daemon).
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
                    let is_active = move || view.effort.get() == value;
                    view! {
                        <button
                            class="segment"
                            class:active=is_active
                            aria-pressed=move || if is_active() { "true" } else { "false" }
                            on:click=move |_| engine.with_value(|e| e.borrow().set_effort(value))
                        >
                            {label}
                        </button>
                    }
                })
                .collect_view()}
        </div>
    }
}

/// A one-line summary of the effective selection for the panel header.
fn current_selection_summary(view: ViewSignals) -> String {
    let models = view.models.get();
    let Some(active) = view.active_model.get() else {
        return "No model selected — the daemon will pick the interactive default.".to_string();
    };
    let label = label_for(&active, &models);
    let stored = view.stored_selection.get();
    let default = view.default_model.get();
    let qualifier = if stored.as_ref().map(model::stored_to_selected).as_ref() == Some(&active) {
        " \u{00b7} pinned to this conversation"
    } else if default.as_ref() == Some(&active) {
        " \u{00b7} interactive default"
    } else {
        ""
    };
    format!("{label}{qualifier}")
}

/// The compact label for the chat-header model pill.
pub fn model_button_label(models: &[ModelListing], active: &Option<SelectedModel>) -> String {
    match active {
        Some(sel) => short_label(sel, models),
        None => "Model".to_string(),
    }
}

/// Full label ("Name \u{b7} Connection") for a selection, falling back to raw
/// ids when the daemon no longer lists the pair (e.g. connection removed).
fn label_for(sel: &SelectedModel, models: &[ModelListing]) -> String {
    match find(sel, models) {
        Some(listing) => format!(
            "{} \u{00b7} {}",
            short_name(listing),
            listing.connection_label
        ),
        None => format!("{} \u{00b7} {}", sel.model_id, sel.connection_id),
    }
}

/// Just the model's display name (for the tight header pill).
fn short_label(sel: &SelectedModel, models: &[ModelListing]) -> String {
    match find(sel, models) {
        Some(listing) => short_name(listing).to_string(),
        None => sel.model_id.clone(),
    }
}

fn short_name(listing: &ModelListing) -> &str {
    if listing.model.display_name.is_empty() {
        &listing.model.id
    } else {
        &listing.model.display_name
    }
}

fn find<'a>(sel: &SelectedModel, models: &'a [ModelListing]) -> Option<&'a ModelListing> {
    models
        .iter()
        .find(|l| l.connection_id == sel.connection_id && l.model.id == sel.model_id)
}

/// Group listings by connection, preserving the daemon's connector order (so a
/// noisy Bedrock connector still flows in the shape it returned) and the first
/// label seen for each connection id.
fn group_by_connection(models: &[ModelListing]) -> Vec<(String, String, Vec<ModelListing>)> {
    let mut groups: Vec<(String, String, Vec<ModelListing>)> = Vec::new();
    for listing in models {
        match groups
            .iter_mut()
            .find(|(id, _, _)| id == &listing.connection_id)
        {
            Some((_, _, items)) => items.push(listing.clone()),
            None => groups.push((
                listing.connection_id.clone(),
                listing.connection_label.clone(),
                vec![listing.clone()],
            )),
        }
    }
    groups
}
