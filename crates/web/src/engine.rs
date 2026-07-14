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

use desktop_assistant_api_model::client::ChatMessage;
use desktop_assistant_api_model::{
    Command, CommandResult, ConnectionView, ConversationModelSelectionView, EffortLevel,
    ModelListing, PurposeConfigView, PurposeKindApi, PurposesView, SendPromptOverride,
};
use futures::channel::mpsc::UnboundedSender;
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use client_ui_common::{
    Effect, SelectedModel, UiMessage, WindowState, interactive_default_from_purposes,
};

use crate::model;
use crate::transport::Transport;

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
            purposes: RwSignal::new(None),
            purpose_connections: RwSignal::new(Vec::new()),
            purpose_models: RwSignal::new(Vec::new()),
            purposes_busy: RwSignal::new(false),
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
            // The message list, streaming buffer, conversation list, context
            // usage, tasks, scratchpad, voice, and client-tool effects are
            // either re-derived in `sync_view` or out of scope for the
            // foundation. Deliberately ignored (their screens land later).
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

fn unexpected(command: &str, result: &CommandResult) -> UiMessage {
    UiMessage::Error(format!("unexpected reply to {command}: {result:?}"))
}
