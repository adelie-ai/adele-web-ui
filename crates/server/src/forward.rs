//! The one piece of real BFF logic: an [`AssistantApiHandler`] that forwards
//! every browser request to the local daemon over the [`Connector`] (UDS) and
//! streams the daemon's events back.
//!
//! Non-streaming commands are a passthrough. For a streaming `SendMessage`, the
//! daemon assigns its own `request_id` (returned in `SendMessageAck`); we route
//! that turn's events off the Connector's signal stream and rewrite the id to
//! the browser's so the SPA correlates against the id it sent.
//!
//! The Connector is shared: one daemon connection multiplexes every browser
//! session, and [`project_turn_event`] demultiplexes a turn's events off that
//! one broadcast stream by matching `request_id` alone. That match is sound
//! only when nothing outside this process chooses the value, so the
//! `Command::SendMessage.turn_id` this handler sends the daemon is always
//! minted here - never the browser's own id. The browser's id still travels,
//! as the trace the forwarded `traceparent` names (see
//! [`browser_traceparent`]), so the browser, the BFF and the daemon still
//! land in one trace; only the daemon-facing correlation id is BFF-owned.

use std::sync::Arc;
use std::time::{Duration, Instant};

use adelie_telemetry::metrics::{self, Label};
use adelie_telemetry::{TraceParent, trace_id_from_uuid};
use desktop_assistant_api_model as api;
use desktop_assistant_application::conversation_subs::ConversationSubscriptions;
use desktop_assistant_application::{ApiError, ApiResult, AssistantApiHandler, EventSink};
use desktop_assistant_client_common::{AssistantCommands, Connector, SignalEvent};
use desktop_assistant_core::ports::transport::current_client_context;
use tracing::Instrument;

use crate::command_kind::command_kind;

pub struct ForwardingHandler {
    connector: Arc<Connector>,
    /// Per-connection browser-session registry (#33). Returned from
    /// [`AssistantApiHandler::conversation_subscriptions`] so the embedded
    /// `ws-interface` dispatcher registers each browser connection's outbound
    /// sink here at connect and records what it's viewing from the SPA's
    /// `SubscribeConversations`. The background event-relay
    /// ([`crate::relay::run_relay`]) fans the daemon's cross-client / background
    /// events to those sessions through it — the same registry, shared.
    subs: Arc<ConversationSubscriptions>,
}

impl ForwardingHandler {
    pub fn new(connector: Arc<Connector>, subs: Arc<ConversationSubscriptions>) -> Self {
        Self { connector, subs }
    }

    fn commands(&self) -> ApiResult<&(dyn AssistantCommands + '_)> {
        self.connector
            .client()
            .as_commands()
            .ok_or_else(|| ApiError::Core("transport has no command channel".to_string()))
    }
}

#[async_trait::async_trait]
impl AssistantApiHandler for ForwardingHandler {
    async fn handle_command(&self, cmd: api::Command) -> ApiResult<api::CommandResult> {
        // Tool-activity messages (tool results, system prompts, empty tool-call
        // assistant turns) are display noise and can be large; strip them from
        // the conversation snapshot before it crosses the VPN to the browser
        // (#58). Only GetConversation carries the full transcript; every other
        // command — including GetMessages, which the Phase-2 opt-in verbose view
        // fetches — passes through untouched. Compute the gate before `cmd` moves
        // into `send_command`.
        let is_get_conversation = matches!(cmd, api::Command::GetConversation { .. });

        // One span per forwarded daemon call (adele-web-ui#91): `kind` is the bare
        // variant name only, never a field value, so this stays INFO-safe under D10
        // regardless of which command it wraps.
        let kind = command_kind(&cmd);
        let span = tracing::info_span!("daemon.call", command = %kind);
        let start = Instant::now();

        let sent = async {
            self.commands()?
                .send_command(cmd)
                .await
                .map_err(|e| ApiError::Core(e.to_string()))
        }
        .instrument(span)
        .await;

        record_daemon_call(kind, start.elapsed(), sent.is_err());

        Ok(browser_conversation_result(is_get_conversation, sent?))
    }

    async fn handle_send_message(
        &self,
        conversation_id: String,
        content: String,
        request_id: String,
        sink: Arc<dyn EventSink>,
    ) -> ApiResult<()> {
        self.handle_send_message_with_override(
            conversation_id,
            content,
            None,
            String::new(),
            request_id,
            None,
            sink,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn handle_send_message_with_override(
        &self,
        conversation_id: String,
        content: String,
        override_selection: Option<api::SendPromptOverride>,
        system_refinement: String,
        request_id: String,
        idempotency_key: Option<String>,
        sink: Arc<dyn EventSink>,
    ) -> ApiResult<()> {
        // One span for the whole forwarded turn (adele-web-ui#91): from the SendMessage
        // that starts it to the terminal event that ends it. `command` never carries the
        // prompt - only the fixed variant name - so this stays INFO-safe under D10.
        // `conversation_id` is an id, which D10 permits at INFO; it is what lets a query
        // return every turn in a conversation without any of them sharing a trace (D13).
        let span = tracing::info_span!(
            "daemon.call",
            command = %"SendMessage",
            conversation_id = %conversation_id,
        );
        let start = Instant::now();

        let result = self
            .send_message_forward(
                conversation_id,
                content,
                override_selection,
                system_refinement,
                request_id,
                idempotency_key,
                sink,
            )
            .instrument(span)
            .await;

        record_daemon_call("SendMessage", start.elapsed(), result.is_err());
        result
    }

    /// Hand the dispatcher the shared browser-session registry (#33). This is the
    /// `ws-interface`'s sanctioned seam for server-initiated pushes: the
    /// dispatcher registers each browser connection's outbound sink here at
    /// connect and applies its `SubscribeConversations`. The background relay
    /// ([`crate::relay::run_relay`]) then fans the daemon's cross-client /
    /// background events to those sessions through this same registry. Returning
    /// `None` (the old default) is what left live sync / scratchpad undelivered.
    fn conversation_subscriptions(&self) -> Option<Arc<ConversationSubscriptions>> {
        Some(Arc::clone(&self.subs))
    }
}

impl ForwardingHandler {
    /// The forwarded-turn body `handle_send_message_with_override` wraps in a
    /// `daemon.call` span. Kept as its own method (rather than inline in the trait
    /// method) so the span and timing wrapper stay a plain, uninstrumented view of what
    /// actually happens.
    #[allow(clippy::too_many_arguments)]
    async fn send_message_forward(
        &self,
        conversation_id: String,
        content: String,
        override_selection: Option<api::SendPromptOverride>,
        system_refinement: String,
        request_id: String,
        idempotency_key: Option<String>,
        sink: Arc<dyn EventSink>,
    ) -> ApiResult<()> {
        // Subscribe before sending so no early chunk is missed.
        let mut rx = self.connector.subscribe();

        // Attach the browser's per-turn client context (#557). The embedded
        // front-door dispatcher installs the browser's `SendMessage.client_context`
        // as the `CLIENT_CONTEXT` task-local around this call; we read it, narrow
        // it to the browser-knowable timezone + OS, and forward it on the daemon
        // `SendMessage`. Reading the per-turn task-local (rather than a per-session
        // map on this shared handler) keeps the context correctly scoped to THIS
        // turn's originating session with no shared state to leak — the BFF
        // multiplexes many browser sessions over one daemon connection, and the
        // BFF's own connection deliberately shares nothing (adele-web-ui#64).
        let client_context = forwarded_client_context(current_client_context());

        // Forward the streaming SendMessage. The daemon's dispatcher
        // special-cases it (it's rejected by `handle_command`) and replies with
        // a `SendMessageAck` whose `request_id` stamps this turn's events.
        let ack = self
            .commands()?
            .send_command(api::Command::SendMessage {
                conversation_id,
                content,
                override_selection,
                system_refinement,
                client_context,
                idempotency_key,
                // Always minted here, never the browser's own id (see the
                // module doc): this is the value `project_turn_event` below
                // demultiplexes the shared Connector's broadcast by, so it
                // must stay a value only this process can choose.
                turn_id: Some(uuid::Uuid::new_v4().to_string()),
                // The browser's trace travels here instead, built from the id
                // the browser minted, so the daemon still joins the same
                // trace the browser started even though `turn_id` above is a
                // different value. `None` when the browser sent nothing
                // usable: an invented trace is worse than none.
                traceparent: browser_traceparent(&request_id),
            })
            .await
            .map_err(|e| ApiError::Core(e.to_string()))?;

        let daemon_request_id = match ack {
            api::CommandResult::SendMessageAck { request_id, .. } => request_id,
            other => {
                return Err(ApiError::Core(format!(
                    "expected SendMessageAck from daemon, got {other:?}"
                )));
            }
        };

        // Route this turn's events back to the browser, rewriting the id. Stop
        // on the terminal event, a dropped client, or a disconnect.
        while let Some(signal) = rx.recv().await {
            if matches!(signal, SignalEvent::Disconnected { .. }) {
                break;
            }
            let Some((event, terminal)) =
                project_turn_event(&signal, &daemon_request_id, &request_id)
            else {
                continue; // a different turn, or a non-streamed signal
            };
            if !sink.emit(event).await {
                break; // browser disconnected
            }
            if terminal {
                break;
            }
        }
        Ok(())
    }
}

/// Project a `SignalEvent` belonging to `daemon_request_id` into the browser
/// `api::Event`, rewriting the correlation id to `browser_request_id`. Returns
/// `(event, is_terminal)`, or `None` when the signal is for another turn or is
/// not a per-turn streamed event.
fn project_turn_event(
    signal: &SignalEvent,
    daemon_request_id: &str,
    browser_request_id: &str,
) -> Option<(api::Event, bool)> {
    let rid = || browser_request_id.to_string();
    match signal {
        SignalEvent::UserMessageAdded {
            conversation_id,
            request_id,
            content,
            idempotency_key,
        } if request_id == daemon_request_id => Some((
            api::Event::UserMessageAdded {
                conversation_id: conversation_id.clone(),
                request_id: rid(),
                content: content.clone(),
                // Forward the daemon's echoed initiating key (#570) unchanged so
                // the browser dedupes its own optimistic bubble by exact match;
                // only the correlation id is rewritten to the browser's.
                idempotency_key: idempotency_key.clone(),
            },
            false,
        )),
        SignalEvent::Chunk {
            conversation_id,
            request_id,
            chunk,
        } if request_id == daemon_request_id => Some((
            api::Event::AssistantDelta {
                conversation_id: conversation_id.clone(),
                request_id: rid(),
                chunk: chunk.clone(),
            },
            false,
        )),
        SignalEvent::Status {
            conversation_id,
            request_id,
            message,
        } if request_id == daemon_request_id => Some((
            api::Event::AssistantStatus {
                conversation_id: conversation_id.clone(),
                request_id: rid(),
                message: message.clone(),
                // The daemon signal this projects from carries the status text
                // only, so there is no structured capability change to pass on.
                capability_change: None,
            },
            false,
        )),
        SignalEvent::ContextUsage {
            conversation_id,
            request_id,
            used_tokens,
            budget_tokens,
            compaction_active,
        } if request_id == daemon_request_id => Some((
            api::Event::ContextUsage {
                conversation_id: conversation_id.clone(),
                request_id: rid(),
                used_tokens: *used_tokens,
                budget_tokens: *budget_tokens,
                compaction_active: *compaction_active,
            },
            false,
        )),
        SignalEvent::Complete {
            conversation_id,
            request_id,
            full_response,
        } if request_id == daemon_request_id => Some((
            api::Event::AssistantCompleted {
                conversation_id: conversation_id.clone(),
                request_id: rid(),
                full_response: full_response.clone(),
            },
            true,
        )),
        SignalEvent::Error {
            conversation_id,
            request_id,
            error,
        } if request_id == daemon_request_id => Some((
            api::Event::AssistantError {
                conversation_id: conversation_id.clone(),
                request_id: rid(),
                error: error.clone(),
            },
            true,
        )),
        _ => None,
    }
}

/// Is this a message a reader actually wants to see in the transcript? Matches
/// `client-ui-common`'s default (non-debug) `filter_messages`: user turns and
/// assistant turns that carry visible text. Tool results, system prompts, and
/// empty tool-call-only assistant turns are display noise. Keeping the predicate
/// identical to the shared reducer keeps the web transcript consistent with the
/// gtk/tui clients, which drop the same set client-side (#57).
fn is_display_message(m: &api::MessageView) -> bool {
    match m.role.as_str() {
        "user" => true,
        "assistant" => !m.content.trim().is_empty(),
        _ => false,
    }
}

/// Strip tool-activity messages from a conversation snapshot so the browser
/// never renders raw tool JSON on reload (#58). Conversation metadata (id,
/// title, warnings, model/personality selection) is preserved verbatim — only
/// the message list is narrowed to what a reader wants to see.
fn filter_conversation_tool_activity(mut view: api::ConversationView) -> api::ConversationView {
    view.messages.retain(is_display_message);
    view
}

/// Shape a daemon `CommandResult` for the browser. Today that means stripping
/// tool activity from a `GetConversation` snapshot (#58); every other reply —
/// including `GetMessages`, which the Phase-2 opt-in verbose view uses — passes
/// through untouched. Pure, so `handle_command`'s post-processing is unit-tested
/// without standing up a live daemon.
fn browser_conversation_result(
    is_get_conversation: bool,
    result: api::CommandResult,
) -> api::CommandResult {
    match result {
        api::CommandResult::Conversation(view) if is_get_conversation => {
            api::CommandResult::Conversation(filter_conversation_tool_activity(view))
        }
        other => other,
    }
}

/// Reduce a browser-supplied [`api::ClientContext`] to the fields a browser can
/// legitimately know — its timezone and a coarse OS — forcing every account /
/// device field (`real_name` / `username` / `home_dir` / `hostname`) absent.
///
/// A browser cannot know those, and the BFF must never let a buggy or hostile
/// browser spoof the daemon host's identity into a multi-tenant system prompt
/// (#557): this is the server-side enforcement of "only timezone + OS ever
/// cross to the daemon". Returns `None` when nothing browser-scoped remains, so
/// an empty context forwards `client_context: None`.
fn browser_scoped_client_context(ctx: &api::ClientContext) -> Option<api::ClientContext> {
    let scoped = api::ClientContext {
        timezone: ctx.timezone.clone(),
        os: ctx.os.clone(),
        // Everything a browser cannot know is forced absent regardless of what
        // the browser sent — fail-closed against a spoofed home/username/hostname.
        ..api::ClientContext::default()
    };
    (!scoped.is_empty()).then_some(scoped)
}

/// The per-turn `client_context` to stamp on the `SendMessage` forwarded to the
/// daemon, given the browser's self-reported context for this turn.
///
/// `current` is the `CLIENT_CONTEXT` task-local the front-door dispatcher
/// installs from the browser's `SendMessage.client_context` (#557). `None` in ⇒
/// `None` out (a turn whose session shared no context forwards none); a present
/// context is narrowed to browser-scoped fields via
/// [`browser_scoped_client_context`], so nothing false about the daemon host is
/// ever forwarded.
fn forwarded_client_context(current: Option<api::ClientContext>) -> Option<api::ClientContext> {
    current.as_ref().and_then(browser_scoped_client_context)
}

/// The `traceparent` naming the trace `browser_turn_id` spells, for
/// forwarding to the daemon as a caller-supplied trace to continue.
///
/// A uuid is the same 16 bytes a W3C trace id is, so the browser's own turn
/// id becomes the trace id directly - no lookup table, no second identifier.
/// [`TraceParent::root_for`] is the deterministic root header for a process
/// with no span machinery of its own, which is exactly what a wasm SPA is:
/// the same `browser_turn_id` always produces the same header, so the daemon
/// joins the one trace that id names rather than starting a new one.
///
/// `None` when `browser_turn_id` does not parse as a uuid or is the nil
/// uuid: an invented trace is worse than none, because a receiver that joins
/// a trace nobody started makes the trace wrong rather than absent.
fn browser_traceparent(browser_turn_id: &str) -> Option<String> {
    let uuid = uuid::Uuid::parse_str(browser_turn_id).ok()?;
    let trace_id = trace_id_from_uuid(uuid.into_bytes()).ok()?;
    Some(TraceParent::root_for(trace_id, true).to_header())
}

/// Record the `daemon_call.duration` histogram and, on failure, the
/// `daemon_call.failures` counter for one forwarded daemon call.
fn record_daemon_call(command: &str, elapsed: Duration, failed: bool) {
    let labels = [Label::new("command", command.to_string())];
    metrics::record_duration("daemon_call.duration", elapsed, &labels);
    if failed {
        metrics::increment("daemon_call.failures", &labels);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAEMON_RID: &str = "daemon-req-1";
    const BROWSER_RID: &str = "browser-req-1";
    /// A distinctive reply body the D10 content test looks for. It must never appear in
    /// any captured span or event field.
    const REPLY_MARKER: &str = "REPLY_MARKER_TOKEN_never_at_info";

    fn mv(role: &str, content: &str) -> api::MessageView {
        api::MessageView {
            id: String::new(),
            role: role.to_string(),
            content: content.to_string(),
            idempotency_key: None,
            content_total_bytes: None,
        }
    }

    fn conversation(messages: Vec<api::MessageView>) -> api::ConversationView {
        api::ConversationView {
            id: "c1".to_string(),
            title: "Trip planning".to_string(),
            messages,
            warnings: Vec::new(),
            model_selection: None,
            conversation_personality: None,
            tool_gate_disabled: false,
            omitted_leading_messages: 0,
            title_total_bytes: None,
        }
    }

    fn roles(view: &api::ConversationView) -> Vec<&str> {
        view.messages.iter().map(|m| m.role.as_str()).collect()
    }

    #[test]
    fn filter_drops_tool_role_messages() {
        let view = conversation(vec![
            mv("user", "how long is the drive?"),
            mv("assistant", "About 40 hours."),
            mv("tool", r#"{"route":{"distance_m":4300000}}"#),
        ]);
        let out = filter_conversation_tool_activity(view);
        assert_eq!(roles(&out), vec!["user", "assistant"], "tool row dropped");
    }

    #[test]
    fn filter_drops_empty_tool_call_assistant_turns() {
        // An assistant turn that only carried tool_calls has empty text content.
        let view = conversation(vec![
            mv("user", "plan my trip"),
            mv("assistant", "   "),
            mv("assistant", "Here's the plan."),
        ]);
        let out = filter_conversation_tool_activity(view);
        assert_eq!(roles(&out), vec!["user", "assistant"]);
        assert_eq!(
            out.messages[1].content, "Here's the plan.",
            "the visible assistant turn survives, the empty one is dropped"
        );
    }

    #[test]
    fn filter_keeps_empty_user_message() {
        // The predicate keeps `user` unconditionally — an empty/whitespace user
        // turn is still the user's turn. This pins parity with the shared
        // reducer's `filter_messages`, which also keeps empty user messages, so a
        // future divergence in either direction is caught.
        let view = conversation(vec![mv("user", "   "), mv("assistant", "hi")]);
        let out = filter_conversation_tool_activity(view);
        assert_eq!(roles(&out), vec!["user", "assistant"]);
    }

    #[test]
    fn filter_keeps_user_and_nonempty_assistant() {
        let view = conversation(vec![mv("user", "hi"), mv("assistant", "hello")]);
        let out = filter_conversation_tool_activity(view);
        assert_eq!(roles(&out), vec!["user", "assistant"], "order preserved");
        assert_eq!(out.messages[0].content, "hi");
        assert_eq!(out.messages[1].content, "hello");
    }

    #[test]
    fn filter_drops_system_messages() {
        let view = conversation(vec![mv("system", "You are Adele."), mv("user", "hi")]);
        let out = filter_conversation_tool_activity(view);
        assert_eq!(roles(&out), vec!["user"], "system prompt is not display");
    }

    #[test]
    fn filter_on_empty_conversation_is_empty() {
        let out = filter_conversation_tool_activity(conversation(vec![]));
        assert!(out.messages.is_empty());
    }

    #[test]
    fn filter_preserves_conversation_metadata() {
        let sel = api::ConversationModelSelectionView {
            connection_id: "work".to_string(),
            model_id: "claude".to_string(),
            effort: Some(api::EffortLevel::High),
        };
        let mut view = conversation(vec![
            mv("user", "hi"),
            mv("tool", r#"{"noise":true}"#),
            mv("assistant", "hello"),
        ]);
        view.model_selection = Some(sel.clone());
        view.warnings = vec![api::ConversationWarning::DanglingModelSelection {
            previous_selection: sel.clone(),
            fallback_to: sel.clone(),
        }];
        let input = view.clone();
        let out = filter_conversation_tool_activity(view);
        assert_eq!(out.id, input.id);
        assert_eq!(out.title, input.title);
        assert_eq!(out.warnings, input.warnings, "advisories survive filtering");
        assert_eq!(out.model_selection, input.model_selection);
        assert_eq!(out.conversation_personality, input.conversation_personality);
        assert_eq!(
            roles(&out),
            vec!["user", "assistant"],
            "but tool row is gone"
        );
    }

    #[test]
    fn handle_command_filters_get_conversation_only() {
        let with_tools = conversation(vec![mv("user", "hi"), mv("tool", "{}")]);
        // GetConversation result → filtered.
        let filtered =
            browser_conversation_result(true, api::CommandResult::Conversation(with_tools.clone()));
        match filtered {
            api::CommandResult::Conversation(v) => assert_eq!(roles(&v), vec!["user"]),
            other => panic!("expected Conversation, got {other:?}"),
        }
        // A Conversation from a non-GetConversation command → untouched (the gate
        // is closed), so no reply is silently reshaped.
        let untouched =
            browser_conversation_result(false, api::CommandResult::Conversation(with_tools));
        match untouched {
            api::CommandResult::Conversation(v) => {
                assert_eq!(roles(&v), vec!["user", "tool"], "gate closed: not filtered")
            }
            other => panic!("expected Conversation, got {other:?}"),
        }
        // A non-Conversation reply is passed straight through.
        assert!(matches!(
            browser_conversation_result(true, api::CommandResult::Ack),
            api::CommandResult::Ack
        ));
    }

    fn chunk(request_id: &str) -> SignalEvent {
        SignalEvent::Chunk {
            conversation_id: "c1".to_string(),
            request_id: request_id.to_string(),
            chunk: "hi".to_string(),
        }
    }

    #[test]
    fn matching_chunk_maps_to_delta_with_browser_id_and_is_not_terminal() {
        let (event, terminal) =
            project_turn_event(&chunk(DAEMON_RID), DAEMON_RID, BROWSER_RID).expect("projected");
        assert!(!terminal);
        match event {
            api::Event::AssistantDelta {
                conversation_id,
                request_id,
                chunk,
            } => {
                assert_eq!(conversation_id, "c1");
                // The browser's id is restamped — never the daemon's.
                assert_eq!(request_id, BROWSER_RID);
                assert_eq!(chunk, "hi");
            }
            other => panic!("expected AssistantDelta, got {other:?}"),
        }
    }

    #[test]
    fn chunk_for_another_turn_is_dropped() {
        assert!(project_turn_event(&chunk("some-other-turn"), DAEMON_RID, BROWSER_RID).is_none());
    }

    #[test]
    fn complete_is_terminal() {
        let signal = SignalEvent::Complete {
            conversation_id: "c1".to_string(),
            request_id: DAEMON_RID.to_string(),
            full_response: "done".to_string(),
        };
        let (event, terminal) =
            project_turn_event(&signal, DAEMON_RID, BROWSER_RID).expect("projected");
        assert!(terminal, "Complete must end the stream");
        assert!(
            matches!(event, api::Event::AssistantCompleted { request_id, .. } if request_id == BROWSER_RID)
        );
    }

    #[test]
    fn error_is_terminal() {
        let signal = SignalEvent::Error {
            conversation_id: "c1".to_string(),
            request_id: DAEMON_RID.to_string(),
            error: "boom".to_string(),
        };
        let (_, terminal) =
            project_turn_event(&signal, DAEMON_RID, BROWSER_RID).expect("projected");
        assert!(terminal, "Error must end the stream");
    }

    #[test]
    fn status_and_context_usage_map_but_are_not_terminal() {
        let status = SignalEvent::Status {
            conversation_id: "c1".to_string(),
            request_id: DAEMON_RID.to_string(),
            message: "thinking".to_string(),
        };
        let (event, terminal) =
            project_turn_event(&status, DAEMON_RID, BROWSER_RID).expect("projected");
        assert!(!terminal);
        assert!(matches!(event, api::Event::AssistantStatus { .. }));

        let usage = SignalEvent::ContextUsage {
            conversation_id: "c1".to_string(),
            request_id: DAEMON_RID.to_string(),
            used_tokens: 10,
            budget_tokens: 100,
            compaction_active: false,
        };
        let (event, terminal) =
            project_turn_event(&usage, DAEMON_RID, BROWSER_RID).expect("projected");
        assert!(!terminal);
        assert!(matches!(
            event,
            api::Event::ContextUsage {
                used_tokens: 10,
                ..
            }
        ));
    }

    #[test]
    fn disconnect_is_not_projected_as_a_turn_event() {
        let signal = SignalEvent::Disconnected {
            reason: "socket closed".to_string(),
        };
        assert!(project_turn_event(&signal, DAEMON_RID, BROWSER_RID).is_none());
    }

    // --- Browser-scoped client context (#557) --------------------------------

    /// A context carrying every field, as a hostile/buggy browser might send.
    fn full_context() -> api::ClientContext {
        api::ClientContext {
            real_name: Some("Ada Lovelace".into()),
            username: Some("ada".into()),
            home_dir: Some("/home/ada".into()),
            hostname: Some("analytical-engine".into()),
            timezone: Some("Europe/London".into()),
            os: Some("Ubuntu 24.04".into()),
        }
    }

    #[test]
    fn browser_scope_keeps_only_timezone_and_os() {
        // Acceptance: the BFF constructs a ClientContext with ONLY timezone + OS
        // set, from the session's supplied values.
        let scoped = browser_scoped_client_context(&full_context()).expect("some context");
        assert_eq!(scoped.timezone.as_deref(), Some("Europe/London"));
        assert_eq!(scoped.os.as_deref(), Some("Ubuntu 24.04"));
        assert_eq!(scoped.real_name, None, "browser can't know a real name");
        assert_eq!(scoped.username, None, "browser can't know a username");
        assert_eq!(scoped.home_dir, None, "browser can't know a home dir");
        assert_eq!(scoped.hostname, None, "browser can't know a hostname");
    }

    #[test]
    fn browser_scope_of_empty_is_none() {
        assert_eq!(
            browser_scoped_client_context(&api::ClientContext::default()),
            None
        );
    }

    #[test]
    fn browser_scope_strips_account_fields_even_without_tz_or_os() {
        // A context with ONLY spoofed account/device fields narrows to nothing.
        let spoofed = api::ClientContext {
            username: Some("root".into()),
            home_dir: Some("/root".into()),
            hostname: Some("bff-server".into()),
            ..api::ClientContext::default()
        };
        assert_eq!(browser_scoped_client_context(&spoofed), None);
    }

    #[test]
    fn forwarded_context_of_none_is_none() {
        // Acceptance: a session with no stored context forwards client_context: None.
        assert_eq!(forwarded_client_context(None), None);
    }

    #[test]
    fn forwarded_context_narrows_to_timezone_and_os() {
        let scoped = forwarded_client_context(Some(full_context())).expect("some context");
        assert_eq!(scoped.timezone.as_deref(), Some("Europe/London"));
        assert_eq!(scoped.os.as_deref(), Some("Ubuntu 24.04"));
        assert!(scoped.username.is_none() && scoped.hostname.is_none());
    }

    #[test]
    fn forwarded_context_of_empty_is_none() {
        assert_eq!(
            forwarded_client_context(Some(api::ClientContext::default())),
            None
        );
    }

    // --- #570: the browser's SendMessage.idempotency_key must reach the daemon
    // -------------------------------------------------------------------------
    //
    // An end-to-end forwarding check over the *real* UDS transport: a browser
    // `SendMessage` carrying an idempotency key, fed through `ForwardingHandler`,
    // must arrive at the daemon-side command with that exact key. It uses an
    // in-process UDS daemon double (the pattern client-common's `uds_transport`
    // tests use) whose send handler records the key it receives and then emits a
    // terminal event so the BFF's event-forwarding loop unblocks and returns.

    use desktop_assistant_auth_jwt::UserId;
    use desktop_assistant_client_common::{ConnectionConfig, Connector, TransportMode};
    use desktop_assistant_uds::{UdsAuthValidator, UdsServer, UdsServerConfig};
    use std::sync::Mutex;
    use std::time::Duration;

    /// UDS daemon double: capture the `idempotency_key` and `request_id` the
    /// dispatcher hands the send handler, then emit a terminal
    /// `AssistantCompleted` so the forwarding loop breaks instead of blocking
    /// on the never-ending signal stream.
    struct CapturingDaemon {
        captured: Arc<Mutex<Option<Option<String>>>>,
        /// The `request_id` the handler actually received — the daemon-side
        /// dispatcher's own adopted-or-minted turn id, once it reads the
        /// forwarded `turn_id` field. `None` until a send lands.
        captured_request_id: Arc<Mutex<Option<String>>>,
    }

    #[async_trait::async_trait]
    impl AssistantApiHandler for CapturingDaemon {
        async fn handle_command(&self, _cmd: api::Command) -> ApiResult<api::CommandResult> {
            Err(ApiError::Unsupported)
        }

        async fn handle_send_message(
            &self,
            _conversation_id: String,
            _content: String,
            _request_id: String,
            _sink: Arc<dyn EventSink>,
        ) -> ApiResult<()> {
            // Unreached: the dispatcher's legacy (no-registry) send path routes
            // through `handle_send_message_with_override`, overridden below.
            Ok(())
        }

        async fn handle_send_message_with_override(
            &self,
            conversation_id: String,
            _content: String,
            _override_selection: Option<api::SendPromptOverride>,
            _system_refinement: String,
            request_id: String,
            idempotency_key: Option<String>,
            sink: Arc<dyn EventSink>,
        ) -> ApiResult<()> {
            *self.captured.lock().unwrap() = Some(idempotency_key);
            *self.captured_request_id.lock().unwrap() = Some(request_id.clone());
            sink.emit(api::Event::AssistantCompleted {
                conversation_id,
                request_id,
                full_response: REPLY_MARKER.to_string(),
            })
            .await;
            Ok(())
        }
    }

    /// Accept any handshake token — the test asserts forwarding, not auth.
    struct AllowAllAuth;
    #[async_trait::async_trait]
    impl UdsAuthValidator for AllowAllAuth {
        async fn validate_bearer_token(&self, _token: &str) -> bool {
            true
        }

        /// Identity is part of acceptance: a validator that accepts a token
        /// must name the subject it belongs to, or the daemon refuses the
        /// connection rather than filing it under the shared default identity.
        async fn extract_user_id(&self, _token: &str) -> Option<UserId> {
            Some(UserId::from("test-user"))
        }
    }

    /// Browser-side sink that drops events; the test only cares that the command
    /// reached the daemon, not what streamed back.
    struct NoopSink;
    #[async_trait::async_trait]
    impl EventSink for NoopSink {
        async fn emit(&self, _event: api::Event) -> bool {
            true
        }
    }

    async fn wait_for_socket(path: &std::path::Path) {
        for _ in 0..100 {
            if path.exists() && tokio::net::UnixStream::connect(path).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("uds socket {path:?} did not appear");
    }

    /// Stand up a UDS daemon double behind a real `ForwardingHandler` +
    /// `Connector` and return the handler to drive a send through, the two
    /// capture slots (`idempotency_key`, `request_id`) `CapturingDaemon` fills
    /// in, and `(shutdown, dir)` for the caller's own teardown. Shared by every
    /// test in this module that proves a field reaches the daemon-side command
    /// over the real transport, rather than each standing up its own socket.
    async fn spawn_capturing_daemon(
        label: &str,
    ) -> (
        ForwardingHandler,
        Arc<Mutex<Option<Option<String>>>>,
        Arc<Mutex<Option<String>>>,
        tokio::sync::oneshot::Sender<()>,
        std::path::PathBuf,
    ) {
        let captured: Arc<Mutex<Option<Option<String>>>> = Arc::new(Mutex::new(None));
        let captured_request_id: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

        // The UDS server chmods the socket's PARENT to 0700, so it must be a
        // directory we own — a fresh subdir of the temp dir, never the shared
        // temp dir itself.
        let dir =
            std::env::temp_dir().join(format!("adele-web-ui-{label}-{}", uuid::Uuid::new_v4()));
        let socket_path = dir.join("d.sock");

        let handler: Arc<dyn AssistantApiHandler> = Arc::new(CapturingDaemon {
            captured: Arc::clone(&captured),
            captured_request_id: Arc::clone(&captured_request_id),
        });
        let auth: Arc<dyn UdsAuthValidator> = Arc::new(AllowAllAuth);
        let server = UdsServer::new(handler, auth, UdsServerConfig::new(socket_path.clone()));
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            let _ = server
                .serve_with_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await;
        });
        wait_for_socket(&socket_path).await;

        // The BFF's real Connector over UDS, wrapped in the forwarding handler.
        let config = ConnectionConfig {
            transport_mode: TransportMode::Uds,
            socket_path: Some(socket_path.clone()),
            ws_jwt: Some("test-token".to_string()),
            share_client_context: false,
            ..ConnectionConfig::default()
        };
        let connector = Arc::new(Connector::connect(&config).await.expect("connect over uds"));
        let subs = Arc::new(ConversationSubscriptions::new());
        let forwarding = ForwardingHandler::new(connector, subs);

        (forwarding, captured, captured_request_id, shutdown_tx, dir)
    }

    #[tokio::test]
    async fn forwarded_send_message_preserves_idempotency_key() {
        let (forwarding, captured, _captured_request_id, shutdown_tx, dir) =
            spawn_capturing_daemon("idem").await;

        // Feed a browser SendMessage carrying an idempotency key through the BFF.
        const KEY: &str = "turn-key-abc";
        tokio::time::timeout(
            Duration::from_secs(5),
            forwarding.handle_send_message_with_override(
                "c1".to_string(),
                "hi".to_string(),
                None,
                String::new(),
                "browser-req-1".to_string(),
                Some(KEY.to_string()),
                Arc::new(NoopSink),
            ),
        )
        .await
        .expect("forwarding did not complete within 5s (terminal event missed?)")
        .expect("forwarding succeeds");

        let _ = shutdown_tx.send(());
        let _ = std::fs::remove_dir_all(&dir);

        // The daemon-side command must carry the browser's exact key.
        let seen = captured.lock().unwrap().clone();
        assert_eq!(
            seen,
            Some(Some(KEY.to_string())),
            "the browser SendMessage.idempotency_key must reach the daemon-side command"
        );
    }

    // --- adele-web-ui trace propagation: the BFF mints its own daemon-facing
    // turn id (it is the value the shared Connector's broadcast is demuxed
    // by, in `project_turn_event` below, so it must stay BFF-owned) and
    // carries the browser's trace in `traceparent` instead. -------------------

    #[tokio::test]
    async fn the_bff_mints_its_own_turn_id() {
        // Two sends that carry the SAME id from the browser's side — a
        // collision, deliberate or coincidental. If the BFF forwarded it as
        // `turn_id` rather than minting its own, the daemon would adopt the
        // identical value for both turns, and `project_turn_event`'s
        // `request_id`-only filter over the shared per-connection broadcast
        // would then deliver each turn's events to the other browser too.
        let colliding_id = uuid::Uuid::new_v4().to_string();
        let (forwarding, _captured, captured_request_id, shutdown_tx, dir) =
            spawn_capturing_daemon("mint").await;

        tokio::time::timeout(
            Duration::from_secs(5),
            forwarding.handle_send_message_with_override(
                "c1".to_string(),
                "hi".to_string(),
                None,
                String::new(),
                colliding_id.clone(),
                None,
                Arc::new(NoopSink),
            ),
        )
        .await
        .expect("forwarding did not complete within 5s (terminal event missed?)")
        .expect("forwarding succeeds");
        let first = captured_request_id.lock().unwrap().clone();

        tokio::time::timeout(
            Duration::from_secs(5),
            forwarding.handle_send_message_with_override(
                "c1".to_string(),
                "hi again".to_string(),
                None,
                String::new(),
                colliding_id.clone(),
                None,
                Arc::new(NoopSink),
            ),
        )
        .await
        .expect("forwarding did not complete within 5s (terminal event missed?)")
        .expect("forwarding succeeds");
        let second = captured_request_id.lock().unwrap().clone();

        let _ = shutdown_tx.send(());
        let _ = std::fs::remove_dir_all(&dir);

        assert!(
            first.is_some() && second.is_some(),
            "both sends must reach the daemon-side handler"
        );
        assert_ne!(
            first, second,
            "two browser sends carrying the same id must not mint the same daemon-side turn id"
        );
    }

    #[test]
    fn the_bff_forwards_the_browser_trace() {
        // The header must name the trace id the browser's own turn id
        // spells, so the daemon joins that exact trace rather than one
        // derived from whatever id the BFF minted for `turn_id` above.
        let browser_turn_id = "550e8400-e29b-41d4-a716-446655440000";
        let header = browser_traceparent(browser_turn_id)
            .expect("a valid, non-nil uuid must produce a traceparent");

        let parsed =
            adelie_telemetry::extract_traceparent(&header).expect("a well-formed traceparent");
        let expected_trace_id = trace_id_from_uuid(
            uuid::Uuid::parse_str(browser_turn_id)
                .expect("test fixture is a valid uuid")
                .into_bytes(),
        )
        .expect("a valid non-nil uuid is a valid trace id");

        assert_eq!(
            parsed.trace_id(),
            expected_trace_id,
            "the traceparent must name the trace id the browser's turn id spells"
        );
    }

    #[tokio::test]
    async fn a_browser_send_without_a_turn_id_still_forwards() {
        // "browser-req-1" is not a uuid — an older SPA (or a caller with
        // nothing usable) hands this method a value that spells no trace.
        assert_eq!(
            browser_traceparent("browser-req-1"),
            None,
            "a non-uuid id must not produce an invented traceparent"
        );

        let (forwarding, _captured, captured_request_id, shutdown_tx, dir) =
            spawn_capturing_daemon("noturnid").await;

        tokio::time::timeout(
            Duration::from_secs(5),
            forwarding.handle_send_message_with_override(
                "c1".to_string(),
                "hi".to_string(),
                None,
                String::new(),
                "browser-req-1".to_string(),
                None,
                Arc::new(NoopSink),
            ),
        )
        .await
        .expect("forwarding did not complete within 5s (terminal event missed?)")
        .expect("forwarding must still succeed with no adoptable browser id");

        let _ = shutdown_tx.send(());
        let _ = std::fs::remove_dir_all(&dir);

        // The BFF must still mint its own turn_id and the send must still
        // reach the daemon — an unusable browser id is not a refusal reason.
        assert!(
            captured_request_id.lock().unwrap().is_some(),
            "the BFF must still mint a turn_id and the send must still reach the daemon"
        );
    }

    // --- adele-web-ui#91: a span per forwarded daemon call, content-free (D10) --------
    // `command_kind` itself is tested in its own module (`crate::command_kind`).

    /// Acceptance (epic D10): the prompt a browser sends and the reply the daemon
    /// returns never reach an INFO-level span field. Drives `handle_send_message_with_
    /// override` over a real UDS daemon double (the same pattern as the idempotency-key
    /// test above), with a tracing capture layer installed so the test reads back what
    /// the `daemon.call` span actually recorded instead of trusting the code that wrote
    /// it.
    #[tokio::test]
    async fn no_request_or_reply_body_at_info() {
        use crate::test_support::Recorder;
        use tracing_subscriber::layer::SubscriberExt;

        const PROMPT_MARKER: &str = "PROMPT_MARKER_TOKEN_never_at_info";

        let recorder = Recorder::new();
        let subscriber = tracing_subscriber::registry().with(recorder.clone());
        let _tracing_guard = tracing::subscriber::set_default(subscriber);

        let dir =
            std::env::temp_dir().join(format!("adele-web-ui-content-{}", uuid::Uuid::new_v4()));
        let socket_path = dir.join("d.sock");

        let handler: Arc<dyn AssistantApiHandler> = Arc::new(CapturingDaemon {
            captured: Arc::new(Mutex::new(None)),
            captured_request_id: Arc::new(Mutex::new(None)),
        });
        let auth: Arc<dyn UdsAuthValidator> = Arc::new(AllowAllAuth);
        let server = UdsServer::new(handler, auth, UdsServerConfig::new(socket_path.clone()));
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            let _ = server
                .serve_with_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await;
        });
        wait_for_socket(&socket_path).await;

        let config = ConnectionConfig {
            transport_mode: TransportMode::Uds,
            socket_path: Some(socket_path.clone()),
            ws_jwt: Some("test-token".to_string()),
            share_client_context: false,
            ..ConnectionConfig::default()
        };
        let connector = Arc::new(Connector::connect(&config).await.expect("connect over uds"));
        let subs = Arc::new(ConversationSubscriptions::new());
        let forwarding = ForwardingHandler::new(connector, subs);

        tokio::time::timeout(
            Duration::from_secs(5),
            forwarding.handle_send_message_with_override(
                "c1".to_string(),
                PROMPT_MARKER.to_string(),
                None,
                String::new(),
                "browser-req-1".to_string(),
                None,
                Arc::new(NoopSink),
            ),
        )
        .await
        .expect("forwarding did not complete within 5s (terminal event missed?)")
        .expect("forwarding succeeds");

        let _ = shutdown_tx.send(());
        let _ = std::fs::remove_dir_all(&dir);

        let daemon_call_spans: Vec<_> = recorder
            .spans()
            .into_iter()
            .filter(|span| span.name == "daemon.call")
            .collect();
        assert!(
            !daemon_call_spans.is_empty(),
            "a daemon.call span must wrap the forwarded SendMessage"
        );

        for span in &daemon_call_spans {
            assert_eq!(
                span.fields.get("command").map(String::as_str),
                Some("SendMessage"),
                "the daemon.call span must name the command it wraps"
            );
            for (key, value) in &span.fields {
                assert!(
                    !value.contains(PROMPT_MARKER),
                    "daemon.call span field {key:?} carried the prompt: {value:?}"
                );
                assert!(
                    !value.contains(REPLY_MARKER),
                    "daemon.call span field {key:?} carried the reply: {value:?}"
                );
            }
        }
    }

    /// Acceptance (review finding #3, epic D13): "Carry conversation_id as an
    /// attribute. A conversation is not a trace; it is an attribute that lets one
    /// query return every turn in it." `conversation_id` is an id, which D10 permits
    /// at INFO, so it must reach the `daemon.call` span the forwarded SendMessage turn
    /// wraps.
    #[tokio::test]
    async fn daemon_call_span_carries_conversation_id() {
        use crate::test_support::Recorder;
        use tracing_subscriber::layer::SubscriberExt;

        const CONVERSATION_ID: &str = "conversation-under-test";

        let recorder = Recorder::new();
        let subscriber = tracing_subscriber::registry().with(recorder.clone());
        let _tracing_guard = tracing::subscriber::set_default(subscriber);

        let dir =
            std::env::temp_dir().join(format!("adele-web-ui-convid-{}", uuid::Uuid::new_v4()));
        let socket_path = dir.join("d.sock");

        let handler: Arc<dyn AssistantApiHandler> = Arc::new(CapturingDaemon {
            captured: Arc::new(Mutex::new(None)),
            captured_request_id: Arc::new(Mutex::new(None)),
        });
        let auth: Arc<dyn UdsAuthValidator> = Arc::new(AllowAllAuth);
        let server = UdsServer::new(handler, auth, UdsServerConfig::new(socket_path.clone()));
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            let _ = server
                .serve_with_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await;
        });
        wait_for_socket(&socket_path).await;

        let config = ConnectionConfig {
            transport_mode: TransportMode::Uds,
            socket_path: Some(socket_path.clone()),
            ws_jwt: Some("test-token".to_string()),
            share_client_context: false,
            ..ConnectionConfig::default()
        };
        let connector = Arc::new(Connector::connect(&config).await.expect("connect over uds"));
        let subs = Arc::new(ConversationSubscriptions::new());
        let forwarding = ForwardingHandler::new(connector, subs);

        tokio::time::timeout(
            Duration::from_secs(5),
            forwarding.handle_send_message_with_override(
                CONVERSATION_ID.to_string(),
                "hi".to_string(),
                None,
                String::new(),
                "browser-req-1".to_string(),
                None,
                Arc::new(NoopSink),
            ),
        )
        .await
        .expect("forwarding did not complete within 5s (terminal event missed?)")
        .expect("forwarding succeeds");

        let _ = shutdown_tx.send(());
        let _ = std::fs::remove_dir_all(&dir);

        let daemon_call = recorder
            .spans()
            .into_iter()
            .find(|span| span.name == "daemon.call")
            .expect("a daemon.call span must wrap the forwarded SendMessage");
        assert_eq!(
            daemon_call
                .fields
                .get("conversation_id")
                .map(String::as_str),
            Some(CONVERSATION_ID),
            "the daemon.call span must carry conversation_id (D13), so one query \
             returns every turn in a conversation"
        );
    }
}
