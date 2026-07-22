//! The one piece of real BFF logic: an [`AssistantApiHandler`] that forwards
//! every browser request to the local daemon over the [`Connector`] (UDS) and
//! streams the daemon's events back.
//!
//! Non-streaming commands are a passthrough. For a streaming `SendMessage`, the
//! daemon assigns its own `request_id` (returned in `SendMessageAck`); we route
//! that turn's events off the Connector's signal stream and rewrite the id to
//! the browser's so the SPA correlates against the id it sent.

use std::sync::Arc;

use desktop_assistant_api_model as api;
use desktop_assistant_application::conversation_subs::ConversationSubscriptions;
use desktop_assistant_application::{ApiError, ApiResult, AssistantApiHandler, EventSink};
use desktop_assistant_client_common::{AssistantCommands, Connector, SignalEvent};

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
        let result = self
            .commands()?
            .send_command(cmd)
            .await
            .map_err(|e| ApiError::Core(e.to_string()))?;
        Ok(browser_conversation_result(is_get_conversation, result))
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
        // Subscribe before sending so no early chunk is missed.
        let mut rx = self.connector.subscribe();

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
                client_context: None,
                idempotency_key,
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
        } if request_id == daemon_request_id => Some((
            api::Event::UserMessageAdded {
                conversation_id: conversation_id.clone(),
                request_id: rid(),
                content: content.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;

    const DAEMON_RID: &str = "daemon-req-1";
    const BROWSER_RID: &str = "browser-req-1";

    fn mv(role: &str, content: &str) -> api::MessageView {
        api::MessageView {
            id: String::new(),
            role: role.to_string(),
            content: content.to_string(),
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
}
