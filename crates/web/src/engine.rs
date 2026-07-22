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

// The engine's full surface is exercised by the wasm UI (`app.rs`). On the host
// build `app.rs` is absent, so this module is compiled only to unit-test its
// reducer-driving core (see the `#[cfg(test)]` module below); the methods the
// queue tests don't reach are not truly dead — they drive the real client — so
// dead-code is silenced for the host build alone. The wasm clippy gate keeps
// enforcing it on the target that actually ships.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use std::rc::Rc;

use desktop_assistant_api_model::client::{ChatMessage, ConversationSummary};
use desktop_assistant_api_model::{
    ClientContext, Command, CommandResult, ConnectionConfigView, ConnectionView,
    ConversationModelSelectionView, ConversationPersonalityView, EffortLevel, McpServerView,
    MessageView, ModelListing, PurposeConfigView, PurposeKindApi, PurposesView, ScratchpadNoteView,
    SendPromptOverride, ServiceAccountView,
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

/// A one-shot completion callback for a per-connection model refresh: invoked
/// with `Ok(count)` — the number of models the connection now offers — or
/// `Err(reason)`. The connections panel renders it as an inline result line.
pub type ModelsRefreshed = Rc<dyn Fn(Result<usize, String>)>;

/// A one-shot delivery of a fetched archived-conversation list to the switcher's
/// local signal (issue #49). The archived list is a *view-only* concern the
/// shared reducer deliberately does not model — surfacing it through the reducer
/// would leak archived rows into the default list — so the engine hands it back
/// via this callback instead. `Rc<dyn Fn>` because the engine is `!Send` /
/// single-threaded, matching [`ModelsRefreshed`].
pub type ArchivedLoaded = Rc<dyn Fn(Vec<ConversationSummary>)>;

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
    // --- Message queuing (feat/queue-messages) -------------------------------
    /// The live composer text. Engine-owned (not a component-local signal) so the
    /// reducer can push into it via [`Effect::SetComposerText`] — clearing it when
    /// a submit is queued, and loading a queued message back for editing (recall).
    /// The `<input>` binds `prop:value` to this and writes it back `on:input`.
    pub composer: RwSignal<String>,
    /// The open conversation's queued (submitted-while-busy, not-yet-sent)
    /// messages in submit order, pulled from `queued_messages_for_view()` in
    /// `sync_view` after every dispatch. Drives the queued-chips strip + the
    /// "N queued" count. Empty when nothing is queued.
    pub queued: RwSignal<Vec<String>>,
    /// The outbox index of the queued message currently checked out into the
    /// composer for editing (from `editing_queued_index()`), or `None` when
    /// composing fresh. Drives the up/down recall walk; the checked-out item is
    /// absent from `queued` while it is being edited.
    pub editing_queued: RwSignal<Option<usize>>,
    // --- Read-aloud (issue #18) ----------------------------------------------
    /// The most recently *completed* assistant reply, as `(request_id, text)`,
    /// set on every `StreamComplete` (local or cross-client). The read-aloud
    /// toggle watches this to speak finished replies via the browser's
    /// `SpeechSynthesis`; the `request_id` lets it dedup a re-delivered
    /// completion. `None` until the first reply of the session completes.
    pub last_completed_reply: RwSignal<Option<(String, String)>>,
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
    // reducer's `UiMessage`s. The personality panel (#13) follows the same shape.
    /// The daemon's purpose routing. `None` until the panel first loads it.
    pub purposes: RwSignal<Option<PurposesView>>,
    /// Connections offered in the purpose connection dropdowns.
    pub purpose_connections: RwSignal<Vec<ConnectionView>>,
    /// The *full* model list for the purpose model dropdowns — unlike `models`,
    /// this keeps embedding-only models, which the embedding purpose needs.
    pub purpose_models: RwSignal<Vec<ModelListing>>,
    /// True while a purposes load or save is in flight (drives a busy hint).
    pub purposes_busy: RwSignal<bool>,
    // --- Personality (issue #13) ---------------------------------------------
    // Per-conversation state, not owned by the shared reducer: the panel loads
    // the current conversation's override on demand (`GetConversation`) and
    // persists edits (`SetConversationPersonality`) straight to these signals.
    /// The current conversation's stored personality override, or `None` when it
    /// inherits the global personality. Pre-fills the panel's trait selectors.
    pub personality: RwSignal<Option<desktop_assistant_api_model::ConversationPersonalityView>>,
    /// Whether the override has been fetched at least once — distinguishes "not
    /// yet loaded" from a legitimately-`None` (no) override, so the panel's
    /// load-once guard fires exactly once rather than on every empty override.
    pub personality_loaded: RwSignal<bool>,
    /// True while a personality load or save is in flight (drives the busy hint
    /// and disables Save).
    pub personality_busy: RwSignal<bool>,
    // --- Conversation switcher (issue #12) -----------------------------------
    /// Every (non-archived) conversation for the switcher sidebar, mirrored from
    /// the reducer's `SetConversations` effect. The reducer owns the list; this
    /// is a render mirror, so the drawer never keeps a parallel copy.
    pub conversations: RwSignal<Vec<ConversationSummary>>,
    /// The id of the conversation currently open, mirrored from
    /// `WindowState::current_conversation_id`. Drives the switcher's active-row
    /// highlight.
    pub current_conversation_id: RwSignal<Option<String>>,
    // --- Global personality (issue #17) --------------------------------------
    // The daemon's global default disposition (`Config.personality`), read/written
    // via the transport-level config API (`GetConfig` / `SetConfig`). Not owned by
    // the shared reducer (which doesn't model `Config`); the panel loads it on
    // demand and persists edits straight to these signals, like purposes (#11).
    /// The daemon's global personality (the "Expressive 7" trait levels), or
    /// `None` until first fetched via `GetConfig`. Seeds the Global Personality
    /// panel; every trait always carries a concrete level.
    pub global_personality: RwSignal<Option<desktop_assistant_api_model::PersonalitySettingsView>>,
    /// Whether the global personality has been fetched at least once, so the
    /// panel's load-once guard fires exactly once.
    pub global_personality_loaded: RwSignal<bool>,
    /// True while a global-personality load or save is in flight (drives the busy
    /// hint and disables Save).
    pub global_personality_busy: RwSignal<bool>,
    // --- Conversation scratchpad (issue #16) ---------------------------------
    /// The active conversation's scratchpad notes (Adele's ephemeral working
    /// notes; DA#184/#240). The reducer owns the fetch: it emits
    /// `Effect::FetchScratchpad` on a conversation load/switch and each completed
    /// turn, and folds the result back out as `Effect::SidePaneSetScratchpad`
    /// (active-conversation-guarded); the engine mirrors that into this signal.
    /// A switch clears it (empty) until the fetch returns, so notes never linger
    /// across conversations.
    pub scratchpad: RwSignal<Vec<ScratchpadNoteView>>,
    // --- Knowledge base (issue #19) ------------------------------------------
    // Read-only browse/search of the user's long-term KB, loaded on demand into
    // these signals like the connections/purposes panels (the shared reducer
    // doesn't model KB state). Both the browse (`ListKnowledgeEntries`) and
    // search (`SearchKnowledgeEntries`) commands blind-forward through the BFF.
    /// The current KB result page — the most-recent entries (browse) or a
    /// search's hits. Empty until the panel first loads.
    pub knowledge_entries: RwSignal<Vec<desktop_assistant_api_model::KnowledgeEntryView>>,
    /// True while a KB browse/search RPC is in flight (drives the loading hint).
    pub knowledge_busy: RwSignal<bool>,
    /// Whether the KB list has loaded at least once — distinguishes "loading" from
    /// "loaded, but empty" for the panel's empty state.
    pub knowledge_loaded: RwSignal<bool>,
    /// The last KB-panel error, or `None`. Cleared when a load starts.
    pub knowledge_error: RwSignal<Option<String>>,
    /// A monotonically-bumped counter the engine advances on every live
    /// `KnowledgeChanged` (issue #39): the user's long-term KB changed under us
    /// (a dream-cycle pass or an assistant write). The KB panel — while open —
    /// watches this and re-runs its current view (browse or the active search),
    /// so it live-refreshes without a manual poke. A CLOSED panel has no effect
    /// watching this, so the bump is a cheap no-op then (no needless refetch).
    pub knowledge_epoch: RwSignal<u64>,
    // --- Background tasks (issue #50) ----------------------------------------
    // Unlike the load-on-demand panels, tasks ARE modelled by the shared reducer:
    // the engine mirrors its `Effect::Task*` family (fed from relayed, user-scoped
    // `Event::Task*`) into this signal, so the panel is a plain reactive render of
    // it. An authoritative `ListBackgroundTasks` snapshot seeds it on open and
    // reconciles terminal statuses on completion.
    /// Active + recent background tasks (most-recent-first), kept live: seeded by
    /// `refresh_tasks`' snapshot and updated by the mirrored task effects.
    pub tasks: RwSignal<Vec<desktop_assistant_api_model::TaskView>>,
    /// True while a `ListBackgroundTasks` snapshot is in flight (loading hint).
    pub tasks_busy: RwSignal<bool>,
    /// Whether the list has been snapshotted at least once — distinguishes
    /// "loading" from "loaded, but empty", and gates the completion re-fetch so a
    /// never-opened panel does no work.
    pub tasks_loaded: RwSignal<bool>,
    /// The last tasks-snapshot error, or `None`. Cleared when a load starts.
    pub tasks_error: RwSignal<Option<String>>,
    // --- MCP servers (issue #55) ---------------------------------------------
    // Load-on-demand like the connections/knowledge panels (the shared reducer
    // doesn't model MCP config). Every command blind-forwards through the BFF.
    /// The daemon's MCP servers with their honest status, refreshed on the
    /// panel's open and after every write.
    pub mcp_servers: RwSignal<Vec<McpServerView>>,
    /// True while an MCP list/write RPC is in flight (drives the loading hint and
    /// disables Save).
    pub mcp_busy: RwSignal<bool>,
    /// Whether the list has loaded at least once — distinguishes "loading" from
    /// "loaded, but empty" for the panel's empty state.
    pub mcp_loaded: RwSignal<bool>,
    /// The last MCP-panel error, or `None`. Cleared when a load starts.
    pub mcp_error: RwSignal<Option<String>>,
    /// Reusable outbound OAuth service accounts (epic #477) for the http editor's
    /// account picker. Loaded alongside the server list.
    pub mcp_service_accounts: RwSignal<Vec<ServiceAccountView>>,
    // --- Tool activity (issue #59) -------------------------------------------
    /// Per-device opt-in: when true, the transcript interleaves the active
    /// conversation's tool results (from `tool_activity`) as collapsed rows.
    /// Persisted in localStorage; default off. Tool results are absent from the
    /// default transcript (the reducer's `filter_messages` drops them client-side,
    /// and the BFF also strips them from `GetConversation`, #58), so they reach the
    /// view only through the separate fetch this gates.
    pub show_tool_activity: RwSignal<bool>,
    /// The active conversation's full history (all roles), fetched via
    /// `GetMessages` when `show_tool_activity` is on; the transcript interleaves
    /// its tool rows into the live bubbles by position (see
    /// `tool_activity::interleave_tool_rows`). Empty when the toggle is off or
    /// between fetches.
    pub tool_activity: RwSignal<Vec<MessageView>>,
    // --- Share device info (issue #557) --------------------------------------
    /// Per-device opt-*out*: when true (the default), the SPA attaches the
    /// browser-scoped client context (timezone + coarse OS, see
    /// `crate::device_info`) to each `SendMessage` so replies can use the user's
    /// local time. Persisted in localStorage; read at send time by `spawn_send`.
    /// The two device fields a browser can honestly know are the only thing ever
    /// shared — never name / username / home dir / hostname.
    pub share_device_info: RwSignal<bool>,
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
            composer: RwSignal::new(String::new()),
            queued: RwSignal::new(Vec::new()),
            editing_queued: RwSignal::new(None),
            last_completed_reply: RwSignal::new(None),
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
            personality: RwSignal::new(None),
            personality_loaded: RwSignal::new(false),
            personality_busy: RwSignal::new(false),
            conversations: RwSignal::new(Vec::new()),
            current_conversation_id: RwSignal::new(None),
            global_personality: RwSignal::new(None),
            global_personality_loaded: RwSignal::new(false),
            global_personality_busy: RwSignal::new(false),
            scratchpad: RwSignal::new(Vec::new()),
            knowledge_entries: RwSignal::new(Vec::new()),
            knowledge_busy: RwSignal::new(false),
            knowledge_loaded: RwSignal::new(false),
            knowledge_error: RwSignal::new(None),
            knowledge_epoch: RwSignal::new(0),
            tasks: RwSignal::new(Vec::new()),
            tasks_busy: RwSignal::new(false),
            tasks_loaded: RwSignal::new(false),
            tasks_error: RwSignal::new(None),
            mcp_servers: RwSignal::new(Vec::new()),
            mcp_busy: RwSignal::new(false),
            mcp_loaded: RwSignal::new(false),
            mcp_error: RwSignal::new(None),
            mcp_service_accounts: RwSignal::new(Vec::new()),
            show_tool_activity: RwSignal::new(crate::tool_activity::load_persisted_toggle()),
            tool_activity: RwSignal::new(Vec::new()),
            share_device_info: RwSignal::new(crate::device_info::load_persisted_share_device_info()),
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
    /// The browser-scoped client context (timezone + coarse OS), resolved once at
    /// construction (#557). Stamped on each `SendMessage` while
    /// `view.share_device_info` is on; `None` when the browser reports neither.
    browser_context: Option<ClientContext>,
}

impl Engine {
    pub fn new(view: ViewSignals, ui_tx: UnboundedSender<UiMessage>, label: String) -> Self {
        Self {
            state: WindowState::default(),
            view,
            transport: None,
            ui_tx,
            label,
            browser_context: crate::device_info::resolve_browser_context(),
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
            // Read-aloud (issue #18): surface each completed reply for the toggle
            // to speak. Carries the `request_id` so a re-delivered completion
            // (cross-client echo) can be deduped; the reducer still owns the
            // transcript, this is only a spoken-output tap.
            UiMessage::StreamComplete {
                request_id,
                full_response,
            } => self
                .view
                .last_completed_reply
                .set(Some((request_id.clone(), full_response.clone()))),
            // Live KB refresh (issue #39): the reducer models no KB state (it
            // returns no effect for this), so the engine bumps a knowledge epoch
            // the open KB panel watches to re-fetch. A wrapping counter — only
            // the *change* matters, never the absolute value.
            UiMessage::KnowledgeChanged => {
                self.view.knowledge_epoch.update(|n| *n = n.wrapping_add(1))
            }
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

    // --- Message queuing (feat/queue-messages) -------------------------------
    //
    // Thin dispatchers over the reducer's queue messages, driven by the
    // queued-chips strip (edit/remove affordances) and the composer's up/down
    // recall walk. The reducer owns the outbox mutation and pushes any composer
    // change back via `SetComposerText`; these just forward the intent.

    /// Check out queued item `index` into the composer for editing (a chip's
    /// edit tap, or an ArrowUp/ArrowDown recall step). Any already-checked-out
    /// item returns to the queue unchanged first (reducer contract).
    pub fn edit_queued(&mut self, index: usize) {
        self.dispatch(UiMessage::EditQueued { index });
    }

    /// Drop queued item `index` without sending it (a chip's × affordance).
    pub fn remove_queued(&mut self, index: usize) {
        self.dispatch(UiMessage::RemoveQueued { index });
    }

    /// Abandon an in-progress recall edit: the checked-out message returns to the
    /// queue unchanged and the composer clears (ArrowDown off the last item).
    pub fn cancel_queued_edit(&mut self) {
        self.dispatch(UiMessage::CancelQueuedEdit);
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
    /// never disturbs the open chat or model picker. The same list-only refetch
    /// backs live sync (#15): a `ConversationListChanged` event pushed by another
    /// client drives the reducer to emit `RefetchConversationList`, which the
    /// engine runs via `spawn_refetch_list` — so the sidebar updates live. This
    /// method is the drawer's load-on-open call, a resync backstop for a change
    /// missed while the socket was down.
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

    // --- Conversation management (issue #49) ---------------------------------
    //
    // Rename + archive/unarchive for the switcher, layered over the same
    // conversation plumbing as select/new/delete and reusing the shared reducer
    // where it already models the mutation. Each is gated on a live connection
    // (dropped silently when offline, like `delete_conversation`), with a
    // transport fault surfaced as an error toast.

    /// Rename conversation `id` to `title`. On the daemon's ack, feed
    /// `ConversationRenamed` so the reducer patches the sidebar row; when `id`
    /// is the *open* conversation, also re-fetch it (delivered as
    /// `ConversationReloaded`) so the header title — derived from the open
    /// detail, not the summary list — reflects the change too. `title` is the
    /// already-validated new name (see `conversation_manage::effective_rename`).
    pub fn rename_conversation(&self, id: String, title: String) {
        let Some(transport) = self.transport.clone() else {
            return;
        };
        // Capture "is this the open conversation?" before the await — the
        // borrow of `self.state` can't cross the spawned task boundary.
        let is_current = self.state.current_conversation_id.as_deref() == Some(id.as_str());
        let tx = self.ui_tx.clone();
        spawn_local(async move {
            match transport
                .send_command(Command::RenameConversation {
                    id: id.clone(),
                    title: title.clone(),
                })
                .await
            {
                Ok(CommandResult::Ack) => {
                    let _ = tx.unbounded_send(UiMessage::ConversationRenamed {
                        id: id.clone(),
                        title,
                    });
                    if is_current {
                        // Persist the rename into the open detail via a re-fetch,
                        // so the header updates. `ConversationReloaded` refreshes
                        // the cached detail without disturbing the model picker.
                        if let Ok(CommandResult::Conversation(view)) = transport
                            .send_command(Command::GetConversation { id })
                            .await
                        {
                            let _ = tx.unbounded_send(UiMessage::ConversationReloaded(view.into()));
                        }
                    }
                }
                Ok(other) => {
                    let _ = tx.unbounded_send(unexpected("RenameConversation", &other));
                }
                Err(e) => {
                    let _ =
                        tx.unbounded_send(UiMessage::Error(format!("rename conversation: {e}")));
                }
            }
        });
    }

    /// Archive conversation `id`. On the ack, feed `ConversationDeleted`: the
    /// reducer drops the row from the default (non-archived) list and, when the
    /// archived conversation was the open one, clears the chat and re-homes to
    /// another (or a fresh) conversation via `EnsureActiveConversation`. The
    /// conversation is NOT deleted server-side — `include_archived` still lists
    /// it and [`Self::unarchive_conversation`] restores it — so reusing the
    /// `ConversationDeleted` path (which prunes the row + re-homes the view) is
    /// exactly the right default-list behaviour without a new reducer message.
    pub fn archive_conversation(&self, id: String) {
        let Some(transport) = self.transport.clone() else {
            return;
        };
        let tx = self.ui_tx.clone();
        spawn_local(async move {
            match transport
                .send_command(Command::ArchiveConversation { id: id.clone() })
                .await
            {
                Ok(CommandResult::Ack) => {
                    let _ = tx.unbounded_send(UiMessage::ConversationDeleted { id });
                }
                Ok(other) => {
                    let _ = tx.unbounded_send(unexpected("ArchiveConversation", &other));
                }
                Err(e) => {
                    let _ =
                        tx.unbounded_send(UiMessage::Error(format!("archive conversation: {e}")));
                }
            }
        });
    }

    /// Unarchive conversation `id`, restoring it to the default list. On the
    /// ack, re-fetch the default list (delivered as `ConversationListRefetched`,
    /// so the restored row reappears in the sidebar) and re-fetch the archived
    /// list into `on_loaded` (so the row leaves the archived section).
    pub fn unarchive_conversation(&self, id: String, on_loaded: ArchivedLoaded) {
        let Some(transport) = self.transport.clone() else {
            return;
        };
        let tx = self.ui_tx.clone();
        spawn_local(async move {
            match transport
                .send_command(Command::UnarchiveConversation { id })
                .await
            {
                Ok(CommandResult::Ack) => {
                    // Restore into the default list (a view-only sidebar repaint,
                    // like a live `ConversationListChanged` refetch).
                    if let Ok(CommandResult::Conversations(convs)) =
                        transport.send_command(list_conversations()).await
                    {
                        let _ = tx.unbounded_send(UiMessage::ConversationListRefetched(
                            convs.into_iter().map(Into::into).collect(),
                        ));
                    }
                    // Refresh the archived section so the restored row leaves it.
                    deliver_archived(&transport, &on_loaded).await;
                }
                Ok(other) => {
                    let _ = tx.unbounded_send(unexpected("UnarchiveConversation", &other));
                }
                Err(e) => {
                    let _ =
                        tx.unbounded_send(UiMessage::Error(format!("unarchive conversation: {e}")));
                }
            }
        });
    }

    /// Fetch the archived conversations on demand and deliver them to
    /// `on_loaded` (bypassing the reducer — see [`ArchivedLoaded`]). Used to
    /// populate the switcher's archived section when it is expanded. Silent when
    /// offline or on a transport/shape error (the section just stays empty).
    pub fn fetch_archived_conversations(&self, on_loaded: ArchivedLoaded) {
        let Some(transport) = self.transport.clone() else {
            return;
        };
        spawn_local(async move {
            deliver_archived(&transport, &on_loaded).await;
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
        // Message queue (feat/queue-messages): the reducer owns the outbox +
        // edit state and re-derives it after every dispatch, so we pull the
        // render snapshot here rather than tracking `Effect::SetQueuedMessages`.
        // The composer text is NOT pulled — it's pushed by `SetComposerText`
        // (recall/clear) and written by the user's keystrokes, so re-deriving it
        // from state would fight live typing.
        self.view
            .queued
            .set(self.state.queued_messages_for_view().to_vec());
        self.view
            .editing_queued
            .set(self.state.editing_queued_index());
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
            // --- Message queuing (feat/queue-messages) -----------------------
            // The reducer can only reach the live composer widget through this
            // effect: it clears the composer when a submit is queued/committed
            // and loads a queued message back on recall. The queued *list* is
            // pulled in `sync_view` (`SetQueuedMessages` is deliberately ignored).
            Effect::SetComposerText(text) => self.view.composer.set(text),
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
            // --- Conversation scratchpad (issue #16) -------------------------
            // The reducer drives the pad: `FetchScratchpad` (on load/switch and
            // each completed turn) issues the read RPC; `SidePaneSetScratchpad`
            // carries the fetched notes back (empty on a switch, clearing stale
            // notes until the fetch returns). The engine spawns the fetch and
            // mirrors the notes into the panel's signal.
            Effect::FetchScratchpad(id) => self.spawn_fetch_scratchpad(id),
            Effect::SidePaneSetScratchpad(notes) => self.view.scratchpad.set(notes),
            // --- Background tasks (issue #50) --------------------------------
            // The reducer models the task lifecycle and emits these host-facing
            // effects (the GTK client feeds them into its `TasksModel`); the
            // engine mirrors them into the `tasks` signal the panel renders. The
            // small list mutations live in `crate::tasks`, host-tested.
            Effect::TasksReplaceAll(tasks) => self.view.tasks.set(tasks),
            Effect::TaskStarted(task) => self
                .view
                .tasks
                .update(|list| crate::tasks::upsert(list, task)),
            Effect::TaskProgress { id, progress_hint } => self
                .view
                .tasks
                .update(|list| crate::tasks::apply_progress(list, &id, progress_hint)),
            // The reducer's completion effect carries only the id — not the
            // terminal `Completed`/`Failed`/`Cancelled` status. Re-fetch the
            // authoritative snapshot so the finished task shows its real status
            // and stays visible as "recent", but only once the panel has been
            // opened (the guard: a never-opened panel does no work — an
            // un-guarded completion falls through to the `_` arm below).
            // `RefreshSidePaneTasks` is a GTK conversation-side-pane concern with
            // no web analogue and is likewise ignored there.
            Effect::TaskCompleted { .. } if self.view.tasks_loaded.get_untracked() => {
                self.refresh_tasks()
            }
            // The message list, streaming buffer, per-task logs, voice, and
            // client-tool effects are either re-derived in `sync_view` or out of
            // scope. Deliberately ignored.
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

    /// Refresh one connection's available models, **bypassing** the connector's
    /// model cache (`ListAvailableModels { connection_id: Some(id), refresh:
    /// true }`). Bedrock caches its model list, so this is how a user picks up a
    /// newly-granted model without waiting for the cache to expire; every
    /// connector supports the scoped refresh. `done` reports the model count (or
    /// an error) for the panel's inline result — it deliberately does not touch
    /// the chat's model picker, whose `models` signal is a full-list concern
    /// refreshed on its own (a scoped result would wrongly drop other
    /// connections' models from it).
    pub fn refresh_connection_models(&self, connection_id: String, done: ModelsRefreshed) {
        let Some(transport) = self.transport.clone() else {
            done(Err(
                "Not connected — try again once reconnected.".to_string()
            ));
            return;
        };
        spawn_local(async move {
            match transport
                .send_command(Command::ListAvailableModels {
                    connection_id: Some(connection_id.clone()),
                    refresh: true,
                })
                .await
            {
                Ok(CommandResult::Models(models)) => {
                    // Count only this connection's models — defensive in case the
                    // daemon ever answers a scoped request with the full list.
                    let count = models
                        .iter()
                        .filter(|m| m.connection_id == connection_id)
                        .count();
                    done(Ok(count));
                }
                Ok(other) => done(Err(format!(
                    "unexpected reply to ListAvailableModels: {other:?}"
                ))),
                Err(e) => done(Err(e)),
            }
        });
    }

    // --- MCP servers (issue #55) ---------------------------------------------
    //
    // Self-contained CRUD over the typed command-RPC, mirroring the connections
    // methods: the spawned tasks write the MCP signals directly (single-threaded,
    // like the transport). Every command blind-forwards through the BFF — no BFF
    // change. Writes re-list on success so the panel reflects the daemon's truth.

    /// (Re)load the MCP server list. Sets `mcp_servers` + `mcp_loaded` on
    /// success, or `mcp_error` on failure. Called when the panel opens.
    pub fn refresh_mcp_servers(&self) {
        let Some(transport) = self.transport.clone() else {
            return;
        };
        let view = self.view;
        view.mcp_busy.set(true);
        view.mcp_error.set(None);
        spawn_local(async move {
            if let Err(e) = load_mcp_into(&transport, view).await {
                view.mcp_error
                    .set(Some(format!("Failed to load MCP servers: {e}")));
            }
            view.mcp_busy.set(false);
        });
    }

    /// Load the reusable OAuth service accounts (epic #477) for the http editor's
    /// account picker. Best-effort: a failure leaves the list empty (the picker
    /// shows "no accounts") rather than blocking the panel.
    pub fn refresh_service_accounts(&self) {
        let Some(transport) = self.transport.clone() else {
            return;
        };
        let view = self.view;
        spawn_local(async move {
            // Best-effort: an unexpected reply or transport error leaves the
            // picker empty rather than blocking the panel.
            if let Ok(CommandResult::ServiceAccounts(accounts)) =
                transport.send_command(Command::ListServiceAccounts).await
            {
                view.mcp_service_accounts.set(accounts);
            }
        });
    }

    /// Enable/disable a server, then re-list. `done` reports the outcome so the
    /// panel can surface a failure.
    pub fn set_mcp_enabled(&self, name: String, enabled: bool, done: ActionDone) {
        let Some(transport) = self.transport.clone() else {
            done(Err(
                "Not connected — try again once reconnected.".to_string()
            ));
            return;
        };
        let view = self.view;
        view.mcp_busy.set(true);
        view.mcp_error.set(None);
        spawn_local(async move {
            let cmd = Command::SetMcpServerEnabled { name, enabled };
            let result = ack(transport.send_command(cmd).await, "toggle MCP server");
            if result.is_ok() {
                let _ = load_mcp_into(&transport, view).await;
            }
            view.mcp_busy.set(false);
            done(result);
        });
    }

    /// Remove a server, then re-list. `done` reports the outcome.
    pub fn remove_mcp_server(&self, name: String, done: ActionDone) {
        let Some(transport) = self.transport.clone() else {
            done(Err(
                "Not connected — try again once reconnected.".to_string()
            ));
            return;
        };
        let view = self.view;
        view.mcp_busy.set(true);
        view.mcp_error.set(None);
        spawn_local(async move {
            let cmd = Command::RemoveMcpServer { name };
            let result = ack(transport.send_command(cmd).await, "remove MCP server");
            if result.is_ok() {
                let _ = load_mcp_into(&transport, view).await;
            }
            view.mcp_busy.set(false);
            done(result);
        });
    }

    /// Create or replace a server from a `config_json` descriptor. When a bearer
    /// `secret` is supplied it is written FIRST (`SetMcpSecret`) so the upserted
    /// config can reference it, then the `UpsertMcpServer` lands, then the list
    /// refreshes. `done` reports the outcome — the panel closes its form or shows
    /// the error.
    pub fn save_mcp_server(
        &self,
        config_json: String,
        secret: Option<(String, String)>,
        done: ActionDone,
    ) {
        let Some(transport) = self.transport.clone() else {
            done(Err(
                "Not connected — try again once reconnected.".to_string()
            ));
            return;
        };
        let view = self.view;
        view.mcp_busy.set(true);
        view.mcp_error.set(None);
        spawn_local(async move {
            // Secret value first: the upserted config references it by ref, so
            // it must exist before the server tries to connect.
            if let Some((id, value)) = secret {
                let cmd = crate::mcp::mcp_secret_command(id, value);
                if let Err(e) = ack(transport.send_command(cmd).await, "set MCP secret") {
                    view.mcp_busy.set(false);
                    done(Err(e));
                    return;
                }
            }
            let cmd = Command::UpsertMcpServer { config_json };
            if let Err(e) = ack(transport.send_command(cmd).await, "save MCP server") {
                view.mcp_busy.set(false);
                done(Err(e));
                return;
            }
            let refresh = load_mcp_into(&transport, view).await;
            view.mcp_busy.set(false);
            match refresh {
                Ok(()) => done(Ok(())),
                Err(e) => done(Err(format!("Saved, but reloading the list failed: {e}"))),
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

    // --- Personality (issue #13) ---------------------------------------------
    //
    // Per-conversation, persisted on the daemon (unlike the per-send model
    // override): the panel reads the stored override from `GetConversation` and
    // writes edits with `SetConversationPersonality`. Both blind-forward through
    // the BFF (no BFF change). Writes straight to the view signals, matching the
    // purposes panel; the shared reducer doesn't model per-conversation
    // personality.

    /// Load the current conversation's stored personality override into
    /// `personality`, marking `personality_loaded`. With no active conversation
    /// yet, mark loaded with `None` so the panel shows the (empty) editable form
    /// rather than a perpetual spinner. Called on first open + the Refresh button.
    pub fn refresh_personality(&self) {
        let Some(transport) = self.transport.clone() else {
            return;
        };
        let view = self.view;
        let Some(conversation_id) = self.state.current_conversation_id.clone() else {
            view.personality.set(None);
            view.personality_loaded.set(true);
            return;
        };
        view.personality_busy.set(true);
        spawn_local(async move {
            match transport
                .send_command(Command::GetConversation {
                    id: conversation_id,
                })
                .await
            {
                Ok(CommandResult::Conversation(conversation)) => {
                    view.personality.set(conversation.conversation_personality);
                    view.personality_loaded.set(true);
                }
                Ok(other) => view.toast.set(Some(format!(
                    "unexpected reply to GetConversation: {other:?}"
                ))),
                Err(e) => view.toast.set(Some(format!("load personality: {e}"))),
            }
            view.personality_busy.set(false);
        });
    }

    /// Persist the conversation's personality override via
    /// `SetConversationPersonality`, then reflect the daemon's echoed stored
    /// value. An all-`None` override clears it (the daemon stores `NULL`), which
    /// we mirror as `None` so the panel resets every trait to Global.
    pub fn set_personality(&self, personality: ConversationPersonalityView) {
        let Some(transport) = self.transport.clone() else {
            return;
        };
        let Some(conversation_id) = self.state.current_conversation_id.clone() else {
            return;
        };
        let view = self.view;
        view.personality_busy.set(true);
        spawn_local(async move {
            match transport
                .send_command(Command::SetConversationPersonality {
                    conversation_id,
                    personality,
                })
                .await
            {
                Ok(CommandResult::ConversationPersonality(stored)) => {
                    view.personality.set((!stored.is_empty()).then_some(stored));
                    view.personality_loaded.set(true);
                }
                Ok(other) => view.toast.set(Some(format!(
                    "unexpected reply to SetConversationPersonality: {other:?}"
                ))),
                Err(e) => view.toast.set(Some(format!("save personality: {e}"))),
            }
            view.personality_busy.set(false);
        });
    }

    // --- Global personality (issue #17) --------------------------------------
    //
    // The daemon's global default disposition, read/written via the transport-
    // level config API (`GetConfig` / `SetConfig`) rather than the per-
    // conversation `SetConversationPersonality`: this is the base every
    // conversation inherits. Both blind-forward through the BFF (no BFF change);
    // the shared reducer doesn't model `Config`, so these write straight to the
    // view signals like the purposes / per-conversation personality panels.

    /// Load the daemon's global personality into `global_personality` via
    /// `GetConfig`, marking `global_personality_loaded`. Called on the panel's
    /// first open + its Refresh button. `busy` is set synchronously so the
    /// panel's load-once guard can't kick a second fetch before this resolves.
    pub fn refresh_global_personality(&self) {
        let Some(transport) = self.transport.clone() else {
            return;
        };
        let view = self.view;
        view.global_personality_busy.set(true);
        spawn_local(async move {
            match transport.send_command(Command::GetConfig).await {
                Ok(CommandResult::Config(config)) => {
                    view.global_personality.set(Some(config.personality));
                    view.global_personality_loaded.set(true);
                }
                Ok(other) => view
                    .toast
                    .set(Some(format!("unexpected reply to GetConfig: {other:?}"))),
                Err(e) => view
                    .toast
                    .set(Some(format!("load global personality: {e}"))),
            }
            view.global_personality_busy.set(false);
        });
    }

    /// Persist the global personality via `SetConfig` (all seven traits as a
    /// `ConfigChanges` — a full replace), then re-seed from the daemon's echoed
    /// `Config` so the panel reflects the stored state (dirty → clean). A failure
    /// raises a toast and leaves the loaded view intact.
    pub fn save_global_personality(
        &self,
        personality: desktop_assistant_api_model::PersonalitySettingsView,
    ) {
        let Some(transport) = self.transport.clone() else {
            return;
        };
        let view = self.view;
        view.global_personality_busy.set(true);
        spawn_local(async move {
            let changes = crate::global_personality::changes_from(&personality);
            match transport.send_command(Command::SetConfig { changes }).await {
                Ok(CommandResult::Config(config)) => {
                    view.global_personality.set(Some(config.personality));
                    view.global_personality_loaded.set(true);
                }
                Ok(other) => view
                    .toast
                    .set(Some(format!("unexpected reply to SetConfig: {other:?}"))),
                Err(e) => view
                    .toast
                    .set(Some(format!("save global personality: {e}"))),
            }
            view.global_personality_busy.set(false);
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
        // Attach the browser-scoped client context (timezone + coarse OS) while
        // the "Share device info" toggle is on (#557); read it now, at send time,
        // so flipping the toggle takes effect on the next message. Off ⇒ send
        // nothing (the BFF then forwards `client_context: None`).
        let client_context = self
            .view
            .share_device_info
            .get_untracked()
            .then(|| self.browser_context.clone())
            .flatten();
        let tx = self.ui_tx.clone();
        spawn_local(async move {
            let cmd = Command::SendMessage {
                conversation_id: conversation_id.clone(),
                content: prompt.clone(),
                override_selection,
                system_refinement: system_refinement.unwrap_or_default(),
                client_context,
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

    // --- Conversation scratchpad (issue #16) ---------------------------------

    /// Read `id`'s scratchpad and feed the notes back as
    /// `ConversationScratchpadLoaded`, which the reducer turns into
    /// `SidePaneSetScratchpad` (dropped if the conversation was switched away
    /// while the fetch was in flight). Runs the reducer's `FetchScratchpad`
    /// effect; a missing transport means we're between connections, so it's
    /// dropped (the pane keeps its last notes). Read-only — never mutates.
    fn spawn_fetch_scratchpad(&self, id: String) {
        let Some(transport) = self.transport.clone() else {
            return;
        };
        let tx = self.ui_tx.clone();
        spawn_local(async move {
            match transport
                .send_command(Command::GetConversationScratchpad {
                    conversation_id: id.clone(),
                    max_results: None,
                })
                .await
            {
                Ok(CommandResult::Scratchpad(notes)) => {
                    let _ = tx.unbounded_send(UiMessage::ConversationScratchpadLoaded {
                        conversation_id: id,
                        notes,
                    });
                }
                Ok(other) => {
                    let _ = tx.unbounded_send(unexpected("GetConversationScratchpad", &other));
                }
                Err(e) => {
                    let _ = tx.unbounded_send(UiMessage::Error(format!("load scratchpad: {e}")));
                }
            }
        });
    }

    /// Re-read the active conversation's scratchpad on demand (the panel's
    /// Refresh button) — useful when another client mutated the pad without a
    /// local turn. A no-op when there is no active conversation yet.
    pub fn refresh_scratchpad(&self) {
        if let Some(id) = self.state.current_conversation_id.clone() {
            self.spawn_fetch_scratchpad(id);
        }
    }

    // --- Knowledge base (issue #19) ------------------------------------------
    //
    // Read-only browse/search over the daemon's client-facing KB commands. The
    // shared reducer doesn't model KB state, so — like connections/purposes —
    // both spawns write results straight into the view signals. Both commands
    // blind-forward through the BFF to the daemon (no BFF change). The daemon
    // owns KB *writes* (the assistant's tools + the dream cycle); the web client
    // only reads.

    /// Load the most-recent KB entries (browse mode) into `knowledge_entries`.
    /// Called on the panel's first open and its Refresh / Clear-search actions.
    pub fn refresh_knowledge(&self) {
        self.load_knowledge(Command::ListKnowledgeEntries {
            limit: crate::knowledge::KB_LIMIT,
            offset: 0,
            tag_filter: None,
        });
    }

    /// Run a server-side KB search for `query`, loading the hits into
    /// `knowledge_entries`. The panel passes only non-empty, trimmed queries
    /// (`knowledge::normalize_query`); an empty box browses instead.
    pub fn search_knowledge(&self, query: String) {
        self.load_knowledge(Command::SearchKnowledgeEntries {
            query,
            tag_filter: None,
            limit: crate::knowledge::KB_LIMIT,
        });
    }

    /// Shared spawn behind [`refresh_knowledge`] / [`search_knowledge`]: issue
    /// `command`, expect [`CommandResult::KnowledgeEntries`], and mirror the page
    /// (or an error) into the panel's signals. Last-good results stay visible
    /// while a load is in flight (no flicker); a failure surfaces on the error
    /// banner and leaves them intact. A missing transport means we're between
    /// connections, so the action is dropped.
    fn load_knowledge(&self, command: Command) {
        let Some(transport) = self.transport.clone() else {
            return;
        };
        let view = self.view;
        view.knowledge_busy.set(true);
        view.knowledge_error.set(None);
        spawn_local(async move {
            match transport.send_command(command).await {
                Ok(CommandResult::KnowledgeEntries(entries)) => {
                    view.knowledge_entries.set(entries);
                    view.knowledge_loaded.set(true);
                }
                Ok(other) => {
                    view.knowledge_error.set(Some(format!(
                        "unexpected reply to a knowledge command: {other:?}"
                    )));
                }
                Err(e) => {
                    view.knowledge_error
                        .set(Some(format!("Knowledge base error: {e}")));
                }
            }
            view.knowledge_busy.set(false);
        });
    }

    // --- Tool activity (issue #59) -------------------------------------------

    /// Clear the tool-activity snapshot (on a conversation switch or when the
    /// toggle turns off) so a slow in-flight fetch can't render the previous
    /// conversation's tool output against the new transcript.
    pub fn clear_tool_activity(&self) {
        self.view.tool_activity.set(Vec::new());
    }

    /// Fetch the active conversation's full history for the "show tool activity"
    /// view. Issues `GetMessages { include_roles: [] }` (all roles) — which the
    /// BFF forwards untouched — and mirrors it into `tool_activity`, where the
    /// transcript interleaves the tool rows into the live bubbles by position.
    /// `tail = 0, after_count = -1` means "every message". The reply is applied
    /// only if this conversation is still the open one, so a switch mid-fetch
    /// can't bleed one conversation's tool output into another. A missing
    /// transport (between connections) or a non-matching reply drops the refresh.
    pub fn refresh_tool_activity(&self, conversation_id: String) {
        let Some(transport) = self.transport.clone() else {
            return;
        };
        let view = self.view;
        spawn_local(async move {
            let reply = transport
                .send_command(Command::GetMessages {
                    conversation_id: conversation_id.clone(),
                    tail: 0,
                    after_count: -1,
                    include_roles: Vec::new(),
                })
                .await;
            // Drop the result if the user has since switched conversations.
            if view.current_conversation_id.get_untracked().as_deref()
                != Some(conversation_id.as_str())
            {
                return;
            }
            if let Ok(CommandResult::Messages(mv)) = reply {
                view.tool_activity.set(mv.messages);
            }
        });
    }

    // --- Background tasks (issue #50) ----------------------------------------
    //
    // Tasks ARE reducer state (the `Effect::Task*` arms in `run_effect` mirror
    // the lifecycle into `view.tasks`), but the initial/authoritative list is an
    // RPC. `refresh_tasks` snapshots the daemon's active + recent tasks and feeds
    // them back as `TasksLoaded`, which the reducer turns into `TasksReplaceAll`.
    // Called on the panel's open + Refresh, and again after a completion to
    // reconcile terminal statuses. Blind-forwards through the BFF (no BFF change).

    /// Snapshot the daemon's active + recent background tasks
    /// (`ListBackgroundTasks { include_finished: true }`, most-recent-first) into
    /// `view.tasks` via `TasksLoaded`. Last-good results stay visible while a
    /// load is in flight (no flicker); a failure surfaces on `tasks_error` and
    /// leaves them intact. A missing transport means we're between connections,
    /// so it's dropped. Read-only — never mutates a task.
    pub fn refresh_tasks(&self) {
        let Some(transport) = self.transport.clone() else {
            return;
        };
        let view = self.view;
        let tx = self.ui_tx.clone();
        view.tasks_busy.set(true);
        view.tasks_error.set(None);
        spawn_local(async move {
            match transport
                .send_command(Command::ListBackgroundTasks {
                    include_finished: true,
                    limit: Some(crate::tasks::TASKS_LIMIT),
                })
                .await
            {
                Ok(CommandResult::BackgroundTasks(tasks)) => {
                    view.tasks_loaded.set(true);
                    let _ = tx.unbounded_send(UiMessage::TasksLoaded(tasks));
                }
                Ok(other) => {
                    view.tasks_error.set(Some(format!(
                        "unexpected reply to ListBackgroundTasks: {other:?}"
                    )));
                }
                Err(e) => {
                    view.tasks_error
                        .set(Some(format!("Background tasks error: {e}")));
                }
            }
            view.tasks_busy.set(false);
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

/// Fetch the archived conversations (an `include_archived: true` list — which
/// returns active AND archived — filtered to the archived ones) and hand them to
/// `on_loaded` (issue #49). Shared by [`Engine::fetch_archived_conversations`]
/// and the post-unarchive refresh. Silently leaves the section unchanged on a
/// transport/shape error, mirroring `spawn_refetch_list`.
async fn deliver_archived(transport: &Rc<Transport>, on_loaded: &ArchivedLoaded) {
    if let Ok(CommandResult::Conversations(convs)) = transport
        .send_command(Command::ListConversations {
            max_age_days: None,
            include_archived: true,
        })
        .await
    {
        let archived =
            crate::conversation_manage::archived_only(convs.into_iter().map(Into::into).collect());
        on_loaded(archived);
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

/// Run `ListMcpServers` and store the result into the MCP signals. Returns
/// `Err` (leaving the signals untouched) on a transport/shape error so callers
/// can surface it. Marks `mcp_loaded` on success.
async fn load_mcp_into(transport: &Rc<Transport>, view: ViewSignals) -> Result<(), String> {
    match transport.send_command(Command::ListMcpServers).await {
        Ok(CommandResult::McpServers(servers)) => {
            view.mcp_servers.set(servers);
            view.mcp_loaded.set(true);
            Ok(())
        }
        Ok(other) => Err(format!("unexpected reply to ListMcpServers: {other:?}")),
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

#[cfg(test)]
mod tests {
    //! Host-target tests for the reducer-driving engine core (message queuing,
    //! feat/queue-messages). They construct a real [`Engine`] over a fresh Leptos
    //! reactive owner, drive it with public [`UiMessage`]s, and assert the view
    //! signals the composer/queued-chips strip render from. No transport is set,
    //! so every RPC effect early-returns — the reducer's optimistic state is what
    //! the view reflects, exactly as in the browser before a reply lands.

    use super::*;
    use desktop_assistant_api_model::client::ConversationDetail;
    use futures::channel::mpsc;
    use leptos::prelude::Owner;

    /// A minimal loaded conversation detail (no messages / selection / override).
    fn detail(id: &str) -> ConversationDetail {
        ConversationDetail {
            id: id.to_string(),
            title: format!("Conversation {id}"),
            messages: Vec::new(),
            model_selection: None,
            conversation_personality: None,
        }
    }

    /// A fresh engine + a `Copy` handle to the same view signals, with no
    /// transport (RPC effects early-return). Must run under a set [`Owner`] so
    /// the `RwSignal`s allocate — each test sets one first.
    fn engine_and_view() -> (Engine, ViewSignals) {
        let view = ViewSignals::new();
        let (tx, _rx) = mpsc::unbounded::<UiMessage>();
        (Engine::new(view, tx, "test".to_string()), view)
    }

    /// Open `c1` and start a streaming turn on it, so a subsequent submit is
    /// QUEUED rather than sent (the reducer's while-streaming path).
    fn open_streaming(engine: &mut Engine) {
        engine.dispatch(UiMessage::ConversationLoaded(detail("c1")));
        engine.dispatch(UiMessage::PromptSent {
            task_id: String::new(),
            conversation_id: "c1".to_string(),
        });
    }

    // --- AC1/AC5: editing a queued chip loads it into the composer -----------

    #[test]
    fn engine_edit_queued_loads_message_into_composer_and_updates_queued_signal() {
        let owner = Owner::new();
        owner.set();
        let (mut engine, view) = engine_and_view();
        open_streaming(&mut engine);
        assert!(
            view.streaming_active.get_untracked(),
            "the turn must be streaming so the submit queues"
        );

        // Submitting while busy queues the text and the reducer clears the composer.
        engine.submit_prompt("hello".to_string());
        assert_eq!(view.queued.get_untracked(), vec!["hello".to_string()]);
        assert_eq!(view.composer.get_untracked(), "");

        // Editing the chip must load its text into the composer (via
        // `Effect::SetComposerText` → the `composer` signal) and check it out of
        // the view queue — NOT delete it (a `RemoveQueued` mis-dispatch would
        // leave the composer empty and silently drop the message).
        engine.edit_queued(0);
        assert_eq!(
            view.composer.get_untracked(),
            "hello",
            "edit_queued must load the queued text into the composer"
        );
        assert_eq!(view.editing_queued.get_untracked(), Some(0));
        assert!(
            view.queued.get_untracked().is_empty(),
            "the checked-out item leaves the view queue"
        );
    }

    // --- AC8: a completion mid-edit must not drop the checked-out edit --------

    #[test]
    fn flush_while_editing_queued_preserves_checked_out_edit() {
        let owner = Owner::new();
        owner.set();
        let (mut engine, view) = engine_and_view();
        open_streaming(&mut engine);
        engine.submit_prompt("a".to_string());
        engine.submit_prompt("b".to_string());
        assert_eq!(
            view.queued.get_untracked(),
            vec!["a".to_string(), "b".to_string()]
        );

        // Check out "a" for editing.
        engine.edit_queued(0);
        assert_eq!(view.editing_queued.get_untracked(), Some(0));
        assert_eq!(view.composer.get_untracked(), "a");
        assert_eq!(view.queued.get_untracked(), vec!["b".to_string()]);

        // The streaming reply completes WHILE the edit is checked out. The flush
        // must skip (mid-edit), leaving the checked-out edit intact rather than
        // flushing/dropping it.
        engine.dispatch(UiMessage::StreamComplete {
            request_id: "req-1".to_string(),
            full_response: "reply".to_string(),
        });
        assert!(
            !view.streaming_active.get_untracked(),
            "the completion must have ended the stream (so the flush path ran)"
        );
        assert_eq!(
            view.editing_queued.get_untracked(),
            Some(0),
            "the checked-out edit must survive a completion firing mid-edit"
        );
        assert_eq!(
            view.composer.get_untracked(),
            "a",
            "the edit text stays in the composer"
        );
        assert_eq!(
            view.queued.get_untracked(),
            vec!["b".to_string()],
            "the rest of the queue is untouched"
        );
    }

    // --- AC9: sync_view must not clobber a live composer draft ----------------

    #[test]
    fn sync_view_preserves_live_composer_draft_across_dispatch() {
        let owner = Owner::new();
        owner.set();
        let (mut engine, view) = engine_and_view();
        engine.dispatch(UiMessage::ConversationLoaded(detail("c1")));

        // The user is part-way through typing a fresh message.
        view.composer.set("half-typed draft".to_string());

        // An unrelated dispatch (here a live KB change) re-runs `sync_view`. It
        // deliberately does NOT pull the composer from state — re-deriving it
        // would blank the live draft the user is typing.
        engine.dispatch(UiMessage::KnowledgeChanged);
        assert_eq!(
            view.composer.get_untracked(),
            "half-typed draft",
            "sync_view must leave the live composer draft untouched"
        );
    }
}
