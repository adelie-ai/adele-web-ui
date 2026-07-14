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
        self.commands()?
            .send_command(cmd)
            .await
            .map_err(|e| ApiError::Core(e.to_string()))
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

#[cfg(test)]
mod tests {
    use super::*;

    const DAEMON_RID: &str = "daemon-req-1";
    const BROWSER_RID: &str = "browser-req-1";

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
