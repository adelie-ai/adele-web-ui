//! The reducer-driven application engine.
//!
//! Mirrors the other clients' driver (gtk's `handle_ui_message`, the ffi
//! `Engine`, the tui's `apply_core`): own a single [`WindowState`], feed it
//! [`UiMessage`]s, and execute the [`Effect`]s it returns. RPC effects issue
//! commands over the [`Transport`] and feed their replies back as new
//! `UiMessage`s; the rendered view is **re-derived from `WindowState` accessors**
//! after every dispatch (the tui's model), so the message list, streaming text,
//! and title need no per-effect wiring — only the handful of accessor-less
//! transient effects (status, send-sensitivity) are reflected directly.

use std::rc::Rc;

use desktop_assistant_api_model::client::{ChatMessage, ConversationSummary};
use desktop_assistant_api_model::{
    Command, CommandResult, ConnectionConfigView, ConnectionView, ConversationModelSelectionView,
    EffortLevel, ModelListing, PurposeConfigView, PurposeKindApi, PurposesView, SendPromptOverride,
};
use futures::channel::mpsc::UnboundedSender;
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use client_ui_common::{
    ContextUsageView, Effect, SelectedModel, UiMessage, WindowState,
    interactive_default_from_purposes,
};

use crate::connections::{CredentialAction, secret_command};
use crate::model;
use crate::transport::Transport;

/// A one-shot completion callback for a connections CRUD action (save/delete),
/// invoked on the main thread with `Ok(())` on success or `Err(reason)` on
/// failure. The connections panel uses it to close its form or surface an error
/// (`Rc<dyn Fn>` because the engine is `!Send` and single-threaded).
pub type ActionDone = Rc<dyn Fn(Result<(), String>)>;

/// Reactive mirrors of the `WindowState` slices the UI renders. Every field is a
/// `Copy` signal, so `ViewSignals` is `Copy` and drops cheaply into closures and
/// spawned tasks.
#[derive(Clone, Copy)]
pub struct ViewSignals {
    pub connected: RwSignal<bool>,
    pub status: RwSignal<String>,
    pub title: RwSignal<String>,
    pub messages: RwSignal<Vec<ChatMessage>>,
    pub streaming: RwSignal<String>,
    pub streaming_active: RwSignal<bool>,
    pub send_enabled: RwSignal<bool>,
    // --- Model selection (issue #9) ------------------------------------------
    /// Chat-capable models offered across every healthy connection, refreshed on
    /// each (re)connect. Empty means the picker hides (e.g. no connections).
    pub models: RwSignal<Vec<ModelListing>>,
    /// Whether the model picker should be shown (false when the transport can't
    /// carry per-send overrides, or no models are available).
    pub model_picker_visible: RwSignal<bool>,
    /// The *effective* selection driving the picker button + row highlight: the
    /// conversation's stored selection, or the interactive-purpose default when
    /// it has none. `None` only before connect / when nothing resolves.
    pub active_model: RwSignal<Option<SelectedModel>>,
    /// The resolved interactive-purpose default, used as the fallback selection
    /// for conversations with no stored pin.
    pub default_model: RwSignal<Option<SelectedModel>>,
    /// The conversation's *stored* (pinned) selection, if any — distinct from
    /// `active_model`, which folds in the default. Drives the "pinned vs default"
    /// hint in the panel.
    pub stored_selection: RwSignal<Option<ConversationModelSelectionView>>,
    /// The staged effort for the next send (`None` = defer to the daemon's
    /// per-purpose default). Hydrated from a stored selection on load.
    pub effort: RwSignal<Option<EffortLevel>>,
    /// A one-shot passive toast (e.g. a dangling-model-selection fallback).
    pub toast: RwSignal<Option<String>>,
    // --- Context-window usage (issue #14) ------------------------------------
    /// The active conversation's latest context-window fill, or `None` before
    /// its first turn completes / right after a conversation switch. The reducer
    /// owns the numbers + colour bucket (`ContextUsageView`, DA#341) and paints
    /// only the viewed conversation; the engine mirrors its `SetContextUsage`
    /// effect here for the header indicator to render.
    pub context_usage: RwSignal<Option<ContextUsageView>>,
    // --- Connections (issue #10) ---------------------------------------------
    /// Every configured LLM connection, refreshed on demand by the Connections
    /// panel via `ListConnections`. Panel-only state (not consumed by the chat).
    pub connections: RwSignal<Vec<ConnectionView>>,
    /// True while a connections RPC (list / save / delete) is in flight.
    pub connections_busy: RwSignal<bool>,
    /// The last connections-panel error, or `None`. Cleared when an action starts.
    pub connections_error: RwSignal<Option<String>>,
    /// Whether the list has loaded at least once — distinguishes "loading" from
    /// "loaded, but empty" for the panel's empty state.
    pub connections_loaded: RwSignal<bool>,
    // --- Purposes (issue #11) -------------------------------------------------
    // Web-only view state: the shared reducer doesn't own purpose routing, so the
    // panel's data is loaded on demand and written straight to these signals
    // (see `refresh_purposes` / `set_purpose`) rather than routed through the
    // reducer's `UiMessage`s.
    /// The daemon's purpose routing. `None` until the panel first loads it.
    pub purposes: RwSignal<Option<PurposesView>>,
    /// Connections offered in the purpose connection dropdowns.
    pub purpose_connections: RwSignal<Vec<ConnectionView>>,
    /// The *full* model list for the purpose model dropdowns — unlike `models`,
    /// this keeps embedding-only models, which the embedding purpose needs.
    pub purpose_models: RwSignal<Vec<ModelListing>>,
    /// True while a purposes load or save is in flight (drives a busy hint).
    pub purposes_busy: RwSignal<bool>,
    // --- Conversation switcher (issue #12) -----------------------------------
    /// Every (non-archived) conversation for the switcher sidebar, mirrored from
    /// the reducer's `SetConversations` effect. The reducer owns the list; this
    /// is a render mirror, so the drawer never keeps a parallel copy.
    pub conversations: RwSignal<Vec<ConversationSummary>>,
    /// The id of the conversation currently open, mirrored from
    /// `WindowState::current_conversation_id`. Drives the switcher's active-row
    /// highlight.
    pub current_conversation_id: RwSignal<Option<String>>,
}

impl ViewSignals {
    pub fn new() -> Self {
        Self {
            connected: RwSignal::new(false),
            status: RwSignal::new("Connecting…".to_string()),
            title: RwSignal::new(String::new()),
            messages: RwSignal::new(Vec::new()),
            streaming: RwSignal::new(String::new()),
            streaming_active: RwSignal::new(false),
            send_enabled: RwSignal::new(true),
            models: RwSignal::new(Vec::new()),
            model_picker_visible: RwSignal::new(false),
            active_model: RwSignal::new(None),
            default_model: RwSignal::new(None),
            stored_selection: RwSignal::new(None),
            effort: RwSignal::new(None),
            toast: RwSignal::new(None),
            context_usage: RwSignal::new(None),
            connections: RwSignal::new(Vec::new()),
            connections_busy: RwSignal::new(false),
            connections_error: RwSignal::new(None),
            connections_loaded: RwSignal::new(false),
            purposes: RwSignal::new(None),
            purpose_connections: RwSignal::new(Vec::new()),
            purpose_models: RwSignal::new(Vec::new()),
            purposes_busy: RwSignal::new(false),
            conversations: RwSignal::new(Vec::new()),
            current_conversation_id: RwSignal::new(None),
        }
    }
}

impl Default for ViewSignals {
    fn default() -> Self {
        Self::new()
    }
}

/// Owns the reducer state + the live transport, and bridges them to the view.
pub struct Engine {
    state: WindowState,
    view: ViewSignals,
    transport: Option<Rc<Transport>>,
    ui_tx: UnboundedSender<UiMessage>,
    label: String,
}

impl Engine {
    pub fn new(view: ViewSignals, ui_tx: UnboundedSender<UiMessage>, label: String) -> Self {
        Self {
            state: WindowState::default(),
            view,
            transport: None,
            ui_tx,
            label,
        }
    }

    pub fn set_transport(&mut self, transport: Rc<Transport>) {
        self.transport = Some(transport);
    }

    pub fn clear_transport(&mut self) {
        self.transport = None;
    }

    /// Feed one message through the reducer, run its effects, and refresh the
    /// view. The single entry point — transport events and RPC replies all land
    /// here via the engine channel.
    pub fn dispatch(&mut self, msg: UiMessage) {
        match &msg {
            UiMessage::Connected { .. } => self.view.connected.set(true),
            UiMessage::Disconnected { .. } => self.view.connected.set(false),
            _ => {}
        }
        for effect in self.state.apply(msg) {
            self.run_effect(effect);
        }
        self.sync_view();
    }

    /// Submit composer text as a new turn. The reducer adds the optimistic user
    /// bubble to state, so `sync_view` renders it without extra wiring.
    pub fn submit_prompt(&mut self, prompt: String) {
        self.dispatch(UiMessage::SubmitPrompt { prompt });
    }

    // --- Conversation switcher (issue #12) -----------------------------------
    //
    // Three thin actions over the existing conversation plumbing, driven by the
    // switcher drawer. The list/selection *state* stays in the shared reducer
    // (mirrored into `conversations` / `current_conversation_id`); these only
    // spawn the RPCs, reusing the private connect-time spawns where they exist.

    /// Switch the open conversation to `id`, fetching it as a fresh switch
    /// (`ConversationLoaded` → the reducer's `switch_to`, which caches the
    /// transcript, applies its model selection, and re-subscribes turn events).
    /// A no-op when `id` is already open, so re-tapping the active row doesn't
    /// churn an evict/reload.
    pub fn select_conversation(&self, id: String) {
        if self.state.current_conversation_id.as_deref() == Some(id.as_str()) {
            return;
        }
        self.spawn_get_conversation(id, false);
    }

    /// Start a brand-new conversation and open it, reusing the connect-time
    /// create flow (create → `ConversationCreated` → load → refetch the list).
    pub fn new_conversation(&self) {
        self.spawn_create_conversation();
    }

    /// Re-fetch the conversation list (list-only), delivered as
    /// `ConversationListRefetched` so the reducer repaints ONLY the sidebar and
    /// never disturbs the open chat or model picker. Run when the drawer opens so
    /// it reflects any conversations added/removed by another client (#12's
    /// load-on-open; live push is out of scope, tracked as #15).
    pub fn refresh_conversation_list(&self) {
        self.spawn_refetch_list();
    }

    /// Delete `id`. On the daemon's ack, feed `ConversationDeleted` so the
    /// reducer drops the row (repainting the sidebar) and, if it was the open
    /// conversation, clears the chat and falls back to another (or a fresh) one
    /// via `EnsureActiveConversation`. Dropped silently when offline (the button
    /// is gated on the connection), with the error surfaced on a transport fault.
    pub fn delete_conversation(&self, id: String) {
        let Some(transport) = self.transport.clone() else {
            return;
        };
        let tx = self.ui_tx.clone();
        spawn_local(async move {
            match transport
                .send_command(Command::DeleteConversation { id: id.clone() })
                .await
            {
                Ok(CommandResult::Ack) => {
                    let _ = tx.unbounded_send(UiMessage::ConversationDeleted { id });
                }
                Ok(other) => {
                    let _ = tx.unbounded_send(unexpected("DeleteConversation", &other));
                }
                Err(e) => {
                    let _ =
                        tx.unbounded_send(UiMessage::Error(format!("delete conversation: {e}")));
                }
            }
        });
    }

    /// Re-derive the rendered view from `WindowState` accessors.
    fn sync_view(&self) {
        let conv = self.state.current_conversation();
        self.view
            .messages
            .set(conv.map(|c| c.messages.clone()).unwrap_or_default());
        self.view
            .title
            .set(conv.map(|c| c.title.clone()).unwrap_or_default());
        self.view
            .streaming
            .set(self.state.streaming_buffer().to_string());
        self.view
            .streaming_active
            .set(self.state.streaming_is_active_for_view());
        // The switcher's active-row marker (issue #12). Re-derived after every
        // dispatch alongside the title/messages, so a switch/create/delete moves
        // the highlight without per-effect wiring.
        self.view
            .current_conversation_id
            .set(self.state.current_conversation_id.clone());
    }

    fn run_effect(&mut self, effect: Effect) {
        match effect {
            Effect::EnsureActiveConversation => self.ensure_active_conversation(),
            Effect::LoadConversation(id) => self.spawn_get_conversation(id, false),
            Effect::ReloadConversation(id) => self.spawn_get_conversation(id, true),
            Effect::RefetchConversationList => self.spawn_refetch_list(),
            Effect::SendPrompt {
                conversation_id,
                prompt,
                system_refinement,
            } => self.spawn_send(conversation_id, prompt, system_refinement),
            Effect::SubscribeConversations(ids) => self.spawn_subscribe(ids),
            // Accessor-less transient effects: reflect directly.
            Effect::SetStatusText(text) | Effect::SetChatStatus(text) => self.view.status.set(text),
            Effect::ClearChatStatus => self.view.status.set(String::new()),
            Effect::SetSendSensitive(enabled) => self.view.send_enabled.set(enabled),
            // --- Model selection (issue #9) ----------------------------------
            // The reducer owns the *selection* precedence and emits these view
            // effects; the engine mirrors them into signals the settings panel
            // renders, matching how the GTK `ModelPicker` consumes the same set.
            Effect::SetModels(listings) => self.view.models.set(model::chat_capable(listings)),
            Effect::SetModelPickerVisible(visible) => self.view.model_picker_visible.set(visible),
            Effect::SetDefaultModel(default) => self.set_default_model(default),
            Effect::SetModelSelection(selection) => self.set_model_selection(selection),
            Effect::ShowToast(message) => self.view.toast.set(Some(message)),
            // --- Conversation switcher (issue #12) ---------------------------
            // The reducer owns the list; mirror its repaint into the signal the
            // switcher drawer renders (the active-row marker is mirrored in
            // `sync_view` from `current_conversation_id`).
            Effect::SetConversations(convs) => self.view.conversations.set(convs),
            // --- Context-window usage (issue #14) ----------------------------
            // The reducer emits `Some(view)` for a completed turn on the viewed
            // conversation and `None` when switching away (clearing a stale
            // reading); the engine just mirrors it into the indicator's signal.
            Effect::SetContextUsage(usage) => self.view.context_usage.set(usage),
            // The message list, streaming buffer, tasks, scratchpad, voice, and
            // client-tool effects are either re-derived in `sync_view` or out of
            // scope for the foundation. Deliberately ignored (their screens land
            // later).
            _ => {}
        }
    }

    // --- Model selection (issue #9) ------------------------------------------
    //
    // Selection *precedence* lives in the shared reducer (which emits the
    // `SetModelSelection` / `SetDefaultModel` effects above); the *staged
    // override* — a transport concern the reducer deliberately doesn't own —
    // lives here, mirroring the GTK `ModelPicker`. `active_model` is the
    // effective pick (`stored.or(default)`); `current_override` turns it into
    // the next send's override, so a conversation on the default pins that
    // default on its first message.

    /// Record the resolved interactive-purpose default. Adopts it as the active
    /// selection only when nothing is selected yet, so it never clobbers a
    /// conversation's explicit pick (mirrors gtk `set_default_model`).
    fn set_default_model(&self, default: Option<SelectedModel>) {
        self.view.default_model.set(default.clone());
        if self.view.active_model.get_untracked().is_none() {
            self.view.active_model.set(default);
        }
    }

    /// Apply a conversation's stored selection: the active pick becomes
    /// `stored.or(default)` and the effort selector hydrates from the stored
    /// effort (mirrors gtk `set_selection`, plus effort — which gtk doesn't
    /// surface yet).
    fn set_model_selection(&self, selection: Option<ConversationModelSelectionView>) {
        let stored = selection.as_ref().map(model::stored_to_selected);
        let default = self.view.default_model.get_untracked();
        self.view
            .active_model
            .set(model::resolve_active(stored, default));
        self.view
            .effort
            .set(selection.as_ref().and_then(|s| s.effort));
        self.view.stored_selection.set(selection);
    }

    /// Stage a user-chosen model as the active selection for the next send.
    pub fn set_active_model(&self, selection: SelectedModel) {
        self.view.active_model.set(Some(selection));
    }

    /// Stage the effort for the next send (`None` = daemon per-purpose default).
    pub fn set_effort(&self, effort: Option<EffortLevel>) {
        self.view.effort.set(effort);
    }

    /// Re-fetch the model list, bypassing connector caches (Bedrock). Feeds the
    /// result back as `ModelsLoaded`, which the reducer turns into `SetModels`.
    pub fn refresh_models(&self) {
        let Some(transport) = self.transport.clone() else {
            return;
        };
        let tx = self.ui_tx.clone();
        spawn_local(async move {
            match transport
                .send_command(Command::ListAvailableModels {
                    connection_id: None,
                    refresh: true,
                })
                .await
            {
                Ok(CommandResult::Models(models)) => {
                    let _ = tx.unbounded_send(UiMessage::ModelsLoaded(models));
                }
                Ok(other) => {
                    let _ = tx.unbounded_send(unexpected("ListAvailableModels", &other));
                }
                Err(e) => {
                    let _ = tx.unbounded_send(UiMessage::Error(format!("refresh models: {e}")));
                }
            }
        });
    }

    // --- Connections (issue #10) ---------------------------------------------
    //
    // Self-contained CRUD over the command-RPC: unlike model selection, these
    // aren't in the shared reducer, so the spawned tasks write the connection
    // signals directly (the same single-threaded pattern the transport uses via
    // the engine channel). Save/delete take an [`ActionDone`] the panel uses to
    // close its form or surface an error; the list is refreshed after any write.

    /// (Re)load the connection list. Sets `connections` + `connections_loaded`
    /// on success, or `connections_error` on failure. Called when the panel opens.
    pub fn refresh_connections(&self) {
        let Some(transport) = self.transport.clone() else {
            return;
        };
        let view = self.view;
        view.connections_busy.set(true);
        view.connections_error.set(None);
        spawn_local(async move {
            if let Err(e) = load_connections_into(&transport, view).await {
                view.connections_error
                    .set(Some(format!("Failed to load connections: {e}")));
            }
            view.connections_busy.set(false);
        });
    }

    /// Create (`editing_id == None`) or update a connection, then optionally
    /// set/clear its credential, then refresh the list. Credential writes ride a
    /// separate `SetConnectionSecret` after the config write lands (the daemon
    /// requires the connection to exist first). `done` reports the outcome.
    pub fn save_connection(
        &self,
        editing_id: Option<String>,
        id: String,
        config: ConnectionConfigView,
        credential: Option<CredentialAction>,
        done: ActionDone,
    ) {
        let Some(transport) = self.transport.clone() else {
            done(Err(
                "Not connected — try again once reconnected.".to_string()
            ));
            return;
        };
        let view = self.view;
        view.connections_busy.set(true);
        view.connections_error.set(None);
        spawn_local(async move {
            // The credential (if any) is keyed on the resolved id: the immutable
            // edit id, else the freshly-created one.
            let target_id = editing_id.clone().unwrap_or_else(|| id.clone());
            let cmd = match &editing_id {
                Some(existing) => Command::UpdateConnection {
                    id: existing.clone(),
                    config,
                },
                None => Command::CreateConnection {
                    id: id.clone(),
                    config,
                },
            };
            if let Err(e) = ack(transport.send_command(cmd).await, "save connection") {
                view.connections_busy.set(false);
                done(Err(e));
                return;
            }
            if let Some(action) = credential {
                let cmd = secret_command(target_id, action);
                if let Err(e) = ack(transport.send_command(cmd).await, "set credential") {
                    // Config saved; the credential step failed. Refresh so the
                    // list reflects the saved config, and report the partial fail.
                    let _ = load_connections_into(&transport, view).await;
                    view.connections_busy.set(false);
                    done(Err(format!(
                        "Connection saved, but the credential update failed: {e}"
                    )));
                    return;
                }
            }
            let refresh = load_connections_into(&transport, view).await;
            view.connections_busy.set(false);
            // A refresh failure post-write is non-fatal to the save itself.
            match refresh {
                Ok(()) => done(Ok(())),
                Err(e) => done(Err(format!("Saved, but reloading the list failed: {e}"))),
            }
        });
    }

    /// Delete a connection (optionally forcing referencing purposes back to the
    /// interactive purpose), then refresh the list. `done` reports the outcome —
    /// the panel offers a force retry when a non-force delete is refused.
    pub fn delete_connection(&self, id: String, force: bool, done: ActionDone) {
        let Some(transport) = self.transport.clone() else {
            done(Err(
                "Not connected — try again once reconnected.".to_string()
            ));
            return;
        };
        let view = self.view;
        view.connections_busy.set(true);
        view.connections_error.set(None);
        spawn_local(async move {
            let cmd = Command::DeleteConnection { id, force };
            let result = ack(transport.send_command(cmd).await, "delete connection");
            if result.is_ok() {
                let _ = load_connections_into(&transport, view).await;
            }
            view.connections_busy.set(false);
            done(result);
        });
    }

    // --- Purposes (issue #11) ------------------------------------------------
    //
    // Purpose routing isn't reducer state, so — unlike model selection — the
    // panel's data is loaded on demand and written straight to the view signals.
    // Both commands blind-forward through the BFF to the daemon (no BFF change).

    /// Load the connections, purpose routing, and the full model list the
    /// Purposes panel needs, writing each into its signal. Run when the panel
    /// opens (and via its Refresh button). Non-fatal: a failed step leaves that
    /// signal as-is and raises a toast; the others still populate.
    pub fn refresh_purposes(&self) {
        let Some(transport) = self.transport.clone() else {
            return;
        };
        let view = self.view;
        // Set busy *synchronously* so the panel's "load once on open" guard
        // (`purposes.is_none() && !busy`) can't fire a second load before the
        // spawned task runs.
        view.purposes_busy.set(true);
        spawn_local(async move {
            match transport.send_command(Command::ListConnections).await {
                Ok(CommandResult::Connections(connections)) => {
                    view.purpose_connections.set(connections)
                }
                Ok(other) => view.toast.set(Some(format!(
                    "unexpected reply to ListConnections: {other:?}"
                ))),
                Err(e) => view.toast.set(Some(format!("load connections: {e}"))),
            }
            // The full, unfiltered model list (embedding models included).
            match transport
                .send_command(Command::ListAvailableModels {
                    connection_id: None,
                    refresh: false,
                })
                .await
            {
                Ok(CommandResult::Models(models)) => view.purpose_models.set(models),
                Ok(other) => view.toast.set(Some(format!(
                    "unexpected reply to ListAvailableModels: {other:?}"
                ))),
                Err(e) => view.toast.set(Some(format!("load models: {e}"))),
            }
            match transport.send_command(Command::GetPurposes).await {
                Ok(CommandResult::Purposes(purposes)) => view.purposes.set(Some(*purposes)),
                Ok(other) => view
                    .toast
                    .set(Some(format!("unexpected reply to GetPurposes: {other:?}"))),
                Err(e) => view.toast.set(Some(format!("load purposes: {e}"))),
            }
            view.purposes_busy.set(false);
        });
    }

    /// Persist one purpose's routing via `SetPurpose`, then re-fetch `GetPurposes`
    /// so the panel reflects the daemon's stored, resolved state (a save is a
    /// full replace). A failure raises a toast and leaves the loaded view intact.
    pub fn set_purpose(&self, purpose: PurposeKindApi, config: PurposeConfigView) {
        let Some(transport) = self.transport.clone() else {
            return;
        };
        let view = self.view;
        spawn_local(async move {
            view.purposes_busy.set(true);
            match transport
                .send_command(Command::SetPurpose { purpose, config })
                .await
            {
                Ok(CommandResult::Ack) => {
                    if let Ok(CommandResult::Purposes(purposes)) =
                        transport.send_command(Command::GetPurposes).await
                    {
                        view.purposes.set(Some(*purposes));
                    }
                }
                Ok(other) => view
                    .toast
                    .set(Some(format!("unexpected reply to SetPurpose: {other:?}"))),
                Err(e) => view.toast.set(Some(format!("save purpose: {e}"))),
            }
            view.purposes_busy.set(false);
        });
    }

    /// The override to fold into the next `SendMessage`, from the active
    /// selection + staged effort. `None` leaves the daemon to resolve the
    /// conversation's stored selection or the interactive purpose.
    fn current_override(&self) -> Option<SendPromptOverride> {
        let active = self.view.active_model.get_untracked();
        model::override_for_send(active.as_ref(), self.view.effort.get_untracked())
    }

    /// Auto-open the most-recent conversation, or create one when the list is
    /// empty — mirrors the ffi/gtk `ensure_active_conversation`.
    fn ensure_active_conversation(&self) {
        if let Some(active) = self.state.current_conversation_id.as_deref()
            && self.state.conversations.iter().any(|c| c.id == active)
        {
            return;
        }
        match self.state.conversations.first() {
            Some(conv) => self.spawn_get_conversation(conv.id.clone(), false),
            None => self.spawn_create_conversation(),
        }
    }

    // --- RPC spawns ----------------------------------------------------------
    //
    // Each clones the transport `Rc` + the engine sender and runs off the
    // dispatch path, feeding the reply back as a `UiMessage`. A missing transport
    // means we're between connections — the action is dropped (the reducer/UI
    // gate upstream), except `send`, which rolls its optimistic bubble back.

    /// On connect: load the models + interactive default, then the conversation
    /// list, then announce `Connected` (which flips the UI to online).
    ///
    /// The ordering is deliberate. Models and the resolved default are fetched
    /// **before** the conversation list so that when `ConversationsLoaded`
    /// triggers the active conversation to open, the reducer's
    /// `SetModelSelection` resolves `stored.or(default)` against a default that
    /// is already set — the picker shows a concrete model on first paint rather
    /// than flickering through a placeholder. Model/purpose failures are
    /// non-fatal: the chat still connects, the picker just stays hidden/empty.
    pub fn start_initial_load(&self) {
        let Some(transport) = self.transport.clone() else {
            return;
        };
        let tx = self.ui_tx.clone();
        let label = self.label.clone();
        spawn_local(async move {
            // 1. Available models (non-fatal). `ModelsLoaded` -> `SetModels` +
            //    picker visibility.
            match transport
                .send_command(Command::ListAvailableModels {
                    connection_id: None,
                    refresh: false,
                })
                .await
            {
                Ok(CommandResult::Models(models)) => {
                    let _ = tx.unbounded_send(UiMessage::ModelsLoaded(models));
                }
                Ok(other) => {
                    let _ = tx.unbounded_send(unexpected("ListAvailableModels", &other));
                }
                Err(e) => {
                    let _ = tx.unbounded_send(UiMessage::Error(format!("load models: {e}")));
                }
            }

            // 2. Interactive-purpose default (non-fatal). Derived from
            //    `GetPurposes` and fed as `DefaultModelLoaded` -> `SetDefaultModel`.
            match transport.send_command(Command::GetPurposes).await {
                Ok(CommandResult::Purposes(purposes)) => {
                    let default = interactive_default_from_purposes(&purposes);
                    let _ = tx.unbounded_send(UiMessage::DefaultModelLoaded(default));
                }
                Ok(other) => {
                    let _ = tx.unbounded_send(unexpected("GetPurposes", &other));
                }
                Err(e) => {
                    let _ = tx.unbounded_send(UiMessage::Error(format!("load purposes: {e}")));
                }
            }

            // 3. Conversation list. `ConversationsLoaded` drives the reducer to
            //    open or create an active conversation.
            match transport.send_command(list_conversations()).await {
                Ok(CommandResult::Conversations(convs)) => {
                    let _ = tx.unbounded_send(UiMessage::ConversationsLoaded(
                        convs.into_iter().map(Into::into).collect(),
                    ));
                }
                Ok(other) => {
                    let _ = tx.unbounded_send(unexpected("ListConversations", &other));
                }
                Err(e) => {
                    let _ = tx.unbounded_send(UiMessage::Error(format!("load conversations: {e}")));
                }
            }
            let _ = tx.unbounded_send(UiMessage::Connected { label });
        });
    }

    fn spawn_get_conversation(&self, id: String, reload: bool) {
        let Some(transport) = self.transport.clone() else {
            return;
        };
        let tx = self.ui_tx.clone();
        spawn_local(async move {
            match transport
                .send_command(Command::GetConversation { id })
                .await
            {
                Ok(CommandResult::Conversation(view)) => {
                    let detail = view.into();
                    let _ = tx.unbounded_send(if reload {
                        UiMessage::ConversationReloaded(detail)
                    } else {
                        UiMessage::ConversationLoaded(detail)
                    });
                }
                Ok(other) => {
                    let _ = tx.unbounded_send(unexpected("GetConversation", &other));
                }
                Err(e) => {
                    let _ = tx.unbounded_send(UiMessage::Error(format!("load conversation: {e}")));
                }
            }
        });
    }

    fn spawn_refetch_list(&self) {
        let Some(transport) = self.transport.clone() else {
            return;
        };
        let tx = self.ui_tx.clone();
        spawn_local(async move {
            if let Ok(CommandResult::Conversations(convs)) =
                transport.send_command(list_conversations()).await
            {
                let _ = tx.unbounded_send(UiMessage::ConversationListRefetched(
                    convs.into_iter().map(Into::into).collect(),
                ));
            }
        });
    }

    fn spawn_send(
        &self,
        conversation_id: String,
        prompt: String,
        system_refinement: Option<String>,
    ) {
        let Some(transport) = self.transport.clone() else {
            // No live connection: roll the optimistic bubble back out.
            let _ = self.ui_tx.unbounded_send(UiMessage::SendFailed {
                conversation_id,
                prompt,
            });
            let _ = self.ui_tx.unbounded_send(UiMessage::Error(
                "Not connected — message not sent (your text is preserved).".to_string(),
            ));
            return;
        };
        // Fold in the staged model override (issue #9). The daemon pins it as
        // the conversation's selection, so later turns inherit it — there is no
        // separate "set model" command.
        let override_selection = self.current_override();
        let tx = self.ui_tx.clone();
        spawn_local(async move {
            let cmd = Command::SendMessage {
                conversation_id: conversation_id.clone(),
                content: prompt.clone(),
                override_selection,
                system_refinement: system_refinement.unwrap_or_default(),
                idempotency_key: None,
            };
            match transport.send_command(cmd).await {
                // The turn's events (UserMessageAdded / AssistantDelta / …) stream
                // separately and carry the correlation; the ack just confirms the
                // turn started.
                Ok(CommandResult::SendMessageAck { task_id, .. }) => {
                    let _ = tx.unbounded_send(UiMessage::PromptSent {
                        task_id,
                        conversation_id,
                    });
                }
                Ok(other) => {
                    let _ = tx.unbounded_send(unexpected("SendMessage", &other));
                    let _ = tx.unbounded_send(UiMessage::SendFailed {
                        conversation_id,
                        prompt,
                    });
                }
                Err(e) => {
                    let _ = tx.unbounded_send(UiMessage::Error(format!(
                        "Send error: {e} (your text is preserved)."
                    )));
                    let _ = tx.unbounded_send(UiMessage::SendFailed {
                        conversation_id,
                        prompt,
                    });
                }
            }
        });
    }

    fn spawn_subscribe(&self, ids: Vec<String>) {
        let Some(transport) = self.transport.clone() else {
            return;
        };
        let tx = self.ui_tx.clone();
        spawn_local(async move {
            if let Err(e) = transport
                .send_command(Command::SubscribeConversations {
                    conversation_ids: ids,
                })
                .await
            {
                let _ = tx.unbounded_send(UiMessage::Error(format!("subscribe: {e}")));
            }
        });
    }

    fn spawn_create_conversation(&self) {
        let Some(transport) = self.transport.clone() else {
            return;
        };
        let tx = self.ui_tx.clone();
        spawn_local(async move {
            let create = Command::CreateConversation {
                title: "New Conversation".to_string(),
                tags: Vec::new(),
            };
            let id = match transport.send_command(create).await {
                Ok(CommandResult::ConversationId { id }) => id,
                Ok(other) => {
                    let _ = tx.unbounded_send(unexpected("CreateConversation", &other));
                    return;
                }
                Err(e) => {
                    let _ =
                        tx.unbounded_send(UiMessage::Error(format!("create conversation: {e}")));
                    return;
                }
            };
            let _ = tx.unbounded_send(UiMessage::ConversationCreated { id: id.clone() });
            match transport
                .send_command(Command::GetConversation { id })
                .await
            {
                Ok(CommandResult::Conversation(view)) => {
                    let _ = tx.unbounded_send(UiMessage::ConversationLoaded(view.into()));
                }
                Ok(other) => {
                    let _ = tx.unbounded_send(unexpected("GetConversation", &other));
                }
                Err(e) => {
                    let _ =
                        tx.unbounded_send(UiMessage::Error(format!("load new conversation: {e}")));
                }
            }
            if let Ok(CommandResult::Conversations(convs)) =
                transport.send_command(list_conversations()).await
            {
                let _ = tx.unbounded_send(UiMessage::ConversationsLoaded(
                    convs.into_iter().map(Into::into).collect(),
                ));
            }
        });
    }
}

fn list_conversations() -> Command {
    Command::ListConversations {
        max_age_days: None,
        include_archived: false,
    }
}

/// Run `ListConnections` and store the result into the connection signals.
/// Returns `Err` (leaving the signals untouched) on a transport/shape error so
/// callers can surface it. Marks `connections_loaded` on success.
async fn load_connections_into(transport: &Rc<Transport>, view: ViewSignals) -> Result<(), String> {
    match transport.send_command(Command::ListConnections).await {
        Ok(CommandResult::Connections(conns)) => {
            view.connections.set(conns);
            view.connections_loaded.set(true);
            Ok(())
        }
        Ok(other) => Err(format!("unexpected reply to ListConnections: {other:?}")),
        Err(e) => Err(e),
    }
}

/// Collapse a command reply into `Ok(())` when it is the expected [`CommandResult::Ack`],
/// or a human-readable `Err` for a transport error or an unexpected reply shape.
fn ack(result: Result<CommandResult, String>, what: &str) -> Result<(), String> {
    match result {
        Ok(CommandResult::Ack) => Ok(()),
        Ok(other) => Err(format!("unexpected reply to {what}: {other:?}")),
        Err(e) => Err(e),
    }
}

fn unexpected(command: &str, result: &CommandResult) -> UiMessage {
    UiMessage::Error(format!("unexpected reply to {command}: {result:?}"))
}
