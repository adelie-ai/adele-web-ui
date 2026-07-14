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
use desktop_assistant_api_model::{Command, CommandResult};
use futures::channel::mpsc::UnboundedSender;
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use client_ui_common::{Effect, UiMessage, WindowState};

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
            // The message list, streaming buffer, conversation list, context
            // usage, models, tasks, scratchpad, voice, toasts, and client-tool
            // effects are either re-derived in `sync_view` or out of scope for the
            // foundation. Deliberately ignored (their screens land later).
            _ => {}
        }
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

    /// On connect: load the conversation list, then announce `Connected` (which
    /// flips the UI to online). `ConversationsLoaded` drives the reducer to open
    /// or create an active conversation.
    pub fn start_initial_load(&self) {
        let Some(transport) = self.transport.clone() else {
            return;
        };
        let tx = self.ui_tx.clone();
        let label = self.label.clone();
        spawn_local(async move {
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
        let tx = self.ui_tx.clone();
        spawn_local(async move {
            let cmd = Command::SendMessage {
                conversation_id: conversation_id.clone(),
                content: prompt.clone(),
                override_selection: None,
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
