//! Wire-protocol glue for the BFF WebSocket, kept transport-free so it compiles
//! and unit-tests on the host target.
//!
//! The browser speaks `api-model`'s `WsRequest`/`WsFrame` JSON. An incoming
//! `WsFrame::Event` carries a wire [`Event`] — the BFF has already projected the
//! daemon's `SignalEvent` onto it (see `crates/server/src/forward.rs`). This
//! module maps that wire `Event` onto the shared reducer's [`UiMessage`], the
//! inverse of the BFF's `project_turn_event`. Request/result correlation by `id`
//! lives in the transport that will call this.

use client_ui_common::UiMessage;
use desktop_assistant_api_model::Event;

/// Map a wire [`Event`] onto the shared reducer's [`UiMessage`].
///
/// Returns `None` for events the SPA does not surface yet — background tasks,
/// scratchpad/knowledge sync, client-tool calls, and config pushes — so the
/// caller can drop them. These gain arms as their screens land (each has a
/// ready `UiMessage` counterpart in `client-ui-common`).
pub fn event_to_ui_message(event: Event) -> Option<UiMessage> {
    let msg = match event {
        Event::UserMessageAdded {
            conversation_id,
            request_id,
            content,
        } => UiMessage::UserMessageAdded {
            conversation_id,
            request_id,
            content,
        },
        // The reducer routes streaming events by `request_id` alone, so the
        // carried `conversation_id` is intentionally dropped here.
        Event::AssistantDelta {
            request_id, chunk, ..
        } => UiMessage::StreamChunk { request_id, chunk },
        Event::AssistantCompleted {
            request_id,
            full_response,
            ..
        } => UiMessage::StreamComplete {
            request_id,
            full_response,
        },
        Event::AssistantError {
            request_id, error, ..
        } => UiMessage::StreamError { request_id, error },
        Event::AssistantStatus {
            request_id,
            message,
            ..
        } => UiMessage::AssistantStatus {
            request_id,
            message,
        },
        Event::ContextUsage {
            conversation_id,
            used_tokens,
            budget_tokens,
            compaction_active,
            ..
        } => UiMessage::ContextUsage {
            conversation_id,
            used_tokens,
            budget_tokens,
            compaction_active,
        },
        Event::ConversationListChanged { conversation_id } => {
            UiMessage::ConversationListChanged { conversation_id }
        }
        Event::ConversationTitleChanged {
            conversation_id,
            title,
        } => UiMessage::TitleChanged {
            conversation_id,
            title,
        },
        // A pinned model selection stopped resolving (connection removed / model
        // delisted). The daemon has already cleared it and fallen back; the
        // reducer clears the picker and raises a toast (issue #9). This is the
        // only path that surfaces the warning — `GetConversation`'s `warnings`
        // are dropped by `client-common`'s `ConversationView -> ConversationDetail`
        // conversion, so the live event is what clients act on.
        Event::ConversationWarningEmitted {
            conversation_id,
            warning,
        } => UiMessage::ConversationWarning {
            conversation_id,
            warning,
        },
        // A conversation's scratchpad changed (issue #16): the reducer re-reads
        // it (a `FetchScratchpad`) when it's the active conversation. This is the
        // *live-push* path (another client or a mid-turn tool mutating the pad);
        // the turn-boundary refetch already covers the common case. NOTE: the BFF
        // only forwards per-turn events today (`crates/server/src/forward.rs`), so
        // this arm doesn't fire end-to-end until background events are forwarded
        // — mapping it now keeps the wire↔reducer contract complete and ready.
        Event::ScratchpadChanged { conversation_id } => {
            UiMessage::ScratchpadChanged { conversation_id }
        }
        _ => return None,
    };
    Some(msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use desktop_assistant_api_model::{Command, WsFrame, WsRequest};

    #[test]
    fn send_message_request_serializes_snake_case_and_skips_empty_optionals() {
        let req = WsRequest {
            id: "send-1".to_string(),
            command: Command::SendMessage {
                conversation_id: "c1".to_string(),
                content: "hi".to_string(),
                override_selection: None,
                system_refinement: String::new(),
                idempotency_key: None,
            },
        };
        let v = serde_json::to_value(&req).expect("serializes");
        assert_eq!(v["id"], "send-1");
        let sm = &v["command"]["send_message"];
        assert_eq!(sm["conversation_id"], "c1");
        assert_eq!(sm["content"], "hi");
        // `override_selection` renames to "override"; it and the empty refinement
        // / absent idempotency key are skipped, matching the daemon's wire shape.
        assert!(sm.get("override").is_none(), "override skipped when None");
        assert!(
            sm.get("system_refinement").is_none(),
            "empty refinement skipped"
        );
        assert!(sm.get("idempotency_key").is_none(), "absent key skipped");
    }

    #[test]
    fn event_frame_wire_shape_is_doubly_tagged_and_round_trips() {
        // `WsFrame::Event { event: Event }` is an externally-tagged struct
        // variant: the outer key is the variant tag (`event`), the inner object
        // is the struct's single `event` field — hence the double nesting. The
        // transport gets this right for free by (de)serializing the type; this
        // test pins the exact bytes so a daemon-side reshape can't drift silently.
        let frame = WsFrame::Event {
            event: Event::KnowledgeChanged,
        };
        let s = serde_json::to_string(&frame).expect("serializes");
        assert_eq!(s, r#"{"event":{"event":"knowledge_changed"}}"#);
        assert_eq!(
            serde_json::from_str::<WsFrame>(&s).expect("round-trips"),
            frame
        );
    }

    #[test]
    fn assistant_delta_maps_to_stream_chunk_dropping_conversation_id() {
        let ev = Event::AssistantDelta {
            conversation_id: "c1".to_string(),
            request_id: "r1".to_string(),
            chunk: "hello".to_string(),
        };
        assert!(matches!(
            event_to_ui_message(ev),
            Some(UiMessage::StreamChunk { request_id, chunk })
                if request_id == "r1" && chunk == "hello"
        ));
    }

    #[test]
    fn assistant_completed_maps_to_stream_complete() {
        let ev = Event::AssistantCompleted {
            conversation_id: "c1".to_string(),
            request_id: "r1".to_string(),
            full_response: "done".to_string(),
        };
        assert!(matches!(
            event_to_ui_message(ev),
            Some(UiMessage::StreamComplete { request_id, full_response })
                if request_id == "r1" && full_response == "done"
        ));
    }

    #[test]
    fn assistant_error_maps_to_stream_error() {
        let ev = Event::AssistantError {
            conversation_id: "c1".to_string(),
            request_id: "r1".to_string(),
            error: "boom".to_string(),
        };
        assert!(matches!(
            event_to_ui_message(ev),
            Some(UiMessage::StreamError { request_id, error })
                if request_id == "r1" && error == "boom"
        ));
    }

    #[test]
    fn assistant_status_maps_to_status() {
        let ev = Event::AssistantStatus {
            conversation_id: "c1".to_string(),
            request_id: "r1".to_string(),
            message: "searching".to_string(),
        };
        assert!(matches!(
            event_to_ui_message(ev),
            Some(UiMessage::AssistantStatus { request_id, message })
                if request_id == "r1" && message == "searching"
        ));
    }

    #[test]
    fn user_message_added_preserves_all_fields() {
        let ev = Event::UserMessageAdded {
            conversation_id: "c1".to_string(),
            request_id: "r1".to_string(),
            content: "hi".to_string(),
        };
        assert!(matches!(
            event_to_ui_message(ev),
            Some(UiMessage::UserMessageAdded { conversation_id, request_id, content })
                if conversation_id == "c1" && request_id == "r1" && content == "hi"
        ));
    }

    #[test]
    fn context_usage_drops_request_id_and_keeps_budget() {
        let ev = Event::ContextUsage {
            conversation_id: "c1".to_string(),
            request_id: "r1".to_string(),
            used_tokens: 10,
            budget_tokens: 100,
            compaction_active: true,
        };
        assert!(matches!(
            event_to_ui_message(ev),
            Some(UiMessage::ContextUsage { conversation_id, used_tokens, budget_tokens, compaction_active })
                if conversation_id == "c1" && used_tokens == 10 && budget_tokens == 100 && compaction_active
        ));
    }

    #[test]
    fn conversation_title_changed_maps_to_title_changed() {
        let ev = Event::ConversationTitleChanged {
            conversation_id: "c1".to_string(),
            title: "Renamed".to_string(),
        };
        assert!(matches!(
            event_to_ui_message(ev),
            Some(UiMessage::TitleChanged { conversation_id, title })
                if conversation_id == "c1" && title == "Renamed"
        ));
    }

    #[test]
    fn conversation_list_changed_maps_through() {
        let ev = Event::ConversationListChanged {
            conversation_id: "c1".to_string(),
        };
        assert!(matches!(
            event_to_ui_message(ev),
            Some(UiMessage::ConversationListChanged { conversation_id }) if conversation_id == "c1"
        ));
    }

    #[test]
    fn conversation_warning_emitted_maps_to_typed_warning() {
        use desktop_assistant_api_model::{ConversationModelSelectionView, ConversationWarning};
        let selection = |conn: &str, model: &str| ConversationModelSelectionView {
            connection_id: conn.to_string(),
            model_id: model.to_string(),
            effort: None,
        };
        let ev = Event::ConversationWarningEmitted {
            conversation_id: "c1".to_string(),
            warning: ConversationWarning::DanglingModelSelection {
                previous_selection: selection("gone", "ghost"),
                fallback_to: selection("openai", "gpt-4o"),
            },
        };
        assert!(matches!(
            event_to_ui_message(ev),
            Some(UiMessage::ConversationWarning { conversation_id, warning: ConversationWarning::DanglingModelSelection { .. } })
                if conversation_id == "c1"
        ));
    }

    #[test]
    fn live_sync_event_set_all_map() {
        // Live multi-client sync (#15) depends on the daemon's fanned-out
        // cross-client events reaching the reducer. Pin the exact set the SPA
        // surfaces live so a future reshape of the match (or a new `Event`
        // variant) can't let one silently fall through the `_` arm to `None`:
        // every event here MUST map to a `UiMessage` the reducer acts on
        // (message-added, the streaming trio + status, per-turn usage, and the
        // conversation title/list changes that drive the switcher). This
        // complements the per-variant tests above (which pin each mapping's
        // fields) by asserting the coverage SET as one contract.
        let live_sync_events = [
            Event::UserMessageAdded {
                conversation_id: "c1".to_string(),
                request_id: "r1".to_string(),
                content: "hi".to_string(),
            },
            Event::AssistantDelta {
                conversation_id: "c1".to_string(),
                request_id: "r1".to_string(),
                chunk: "he".to_string(),
            },
            Event::AssistantCompleted {
                conversation_id: "c1".to_string(),
                request_id: "r1".to_string(),
                full_response: "hello".to_string(),
            },
            Event::AssistantError {
                conversation_id: "c1".to_string(),
                request_id: "r1".to_string(),
                error: "boom".to_string(),
            },
            Event::AssistantStatus {
                conversation_id: "c1".to_string(),
                request_id: "r1".to_string(),
                message: "searching".to_string(),
            },
            Event::ContextUsage {
                conversation_id: "c1".to_string(),
                request_id: "r1".to_string(),
                used_tokens: 1,
                budget_tokens: 2,
                compaction_active: false,
            },
            Event::ConversationTitleChanged {
                conversation_id: "c1".to_string(),
                title: "Renamed".to_string(),
            },
            Event::ConversationListChanged {
                conversation_id: "c1".to_string(),
            },
        ];
        for event in live_sync_events {
            let label = format!("{event:?}");
            assert!(
                event_to_ui_message(event).is_some(),
                "live-sync event must map to a UiMessage, but dropped to None: {label}"
            );
        }
    }

    #[test]
    fn scratchpad_changed_maps_through() {
        let ev = Event::ScratchpadChanged {
            conversation_id: "c1".to_string(),
        };
        assert!(matches!(
            event_to_ui_message(ev),
            Some(UiMessage::ScratchpadChanged { conversation_id }) if conversation_id == "c1"
        ));
    }

    #[test]
    fn unsurfaced_event_maps_to_none() {
        // No KB screen in the foundation yet, so `KnowledgeChanged` is dropped by
        // the `_` arm rather than mapped (it has a ready UiMessage counterpart for
        // when the panel lands).
        assert!(event_to_ui_message(Event::KnowledgeChanged).is_none());
    }

    #[test]
    fn error_frame_correlates_by_id() {
        let frame: WsFrame =
            serde_json::from_str(r#"{"error":{"id":"req-9","error":"conversation not found"}}"#)
                .expect("error frame parses");
        match frame {
            WsFrame::Error { id, error } => {
                assert_eq!(id, "req-9");
                assert_eq!(error, "conversation not found");
            }
            other => panic!("expected an error frame, got {other:?}"),
        }
    }

    #[test]
    fn result_frame_carries_correlation_id() {
        let frame: WsFrame =
            serde_json::from_str(r#"{"result":{"id":"req-3","result":{"pong":{"value":"pong"}}}}"#)
                .expect("result frame parses");
        match frame {
            WsFrame::Result { id, .. } => assert_eq!(id, "req-3"),
            other => panic!("expected a result frame, got {other:?}"),
        }
    }
}
