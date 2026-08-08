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
/// Returns `None` for events the SPA does not surface — client-tool calls,
/// config pushes, and per-task logs (`TaskLogAppended`: the tasks panel shows
/// status/progress, not logs) — so the caller can drop them. Remaining arms gain
/// as their screens land (each has a ready `UiMessage` counterpart in
/// `client-ui-common`).
pub fn event_to_ui_message(event: Event) -> Option<UiMessage> {
    let msg = match event {
        Event::UserMessageAdded {
            conversation_id,
            request_id,
            content,
            idempotency_key,
        } => UiMessage::UserMessageAdded {
            conversation_id,
            request_id,
            content,
            // Carry the echoed initiating key (#570) so the initiator dedupes its
            // own optimistic bubble by exact key match, independent of ordering;
            // `None` (voice / another client / a keyless send) falls back to the
            // request_id / content dedupe.
            idempotency_key,
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
        // The user's long-term knowledge base changed (issue #39): a dream-cycle
        // pass or the assistant wrote/edited an entry. The reducer returns no
        // effect for this (the KB browser is a self-contained widget), so the
        // engine bumps a knowledge epoch the open panel watches to re-fetch.
        // Mapping it — rather than dropping it via the `_` arm — is what lets the
        // panel live-update; the BFF relay broadcasts it user-scoped.
        Event::KnowledgeChanged => UiMessage::KnowledgeChanged,
        // Background tasks (issue #50): the reducer models the task lifecycle and
        // turns these into the host-facing `Effect::Task*` family the engine
        // mirrors into its `tasks` signal. The BFF relay broadcasts them
        // user-scoped (they carry no conversation).
        Event::TaskStarted { task } => UiMessage::TaskStarted(task),
        Event::TaskProgress { id, progress_hint } => UiMessage::TaskProgress { id, progress_hint },
        // `UiMessage::TaskCompleted` carries only the id: the reducer's effect
        // then triggers the engine's authoritative re-fetch (which reflects the
        // real terminal `Completed`/`Failed`/`Cancelled` status and keeps the
        // finished task visible as "recent"), so the wire event's `status` /
        // `last_error` are intentionally dropped here.
        Event::TaskCompleted { id, .. } => UiMessage::TaskCompleted { id },
        _ => return None,
    };
    Some(msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use desktop_assistant_api_model::{
        Command, ToolTier, TurnCapabilityReason, WsFrame, WsRequest,
    };

    #[test]
    fn send_message_request_serializes_snake_case_and_skips_empty_optionals() {
        let req = WsRequest {
            id: "send-1".to_string(),
            command: Command::SendMessage {
                conversation_id: "c1".to_string(),
                content: "hi".to_string(),
                override_selection: None,
                system_refinement: String::new(),
                client_context: None,
                idempotency_key: None,
                turn_id: None,
                traceparent: None,
            },
        };
        let v = serde_json::to_value(&req).expect("serializes");
        assert_eq!(v["id"], "send-1");
        let sm = &v["command"]["send_message"];
        assert_eq!(sm["conversation_id"], "c1");
        assert_eq!(sm["content"], "hi");
        // `override_selection` renames to "override"; it and the empty refinement
        // / absent client context / absent idempotency key / absent turn id /
        // absent traceparent are skipped, matching the daemon's wire shape.
        assert!(sm.get("override").is_none(), "override skipped when None");
        assert!(
            sm.get("system_refinement").is_none(),
            "empty refinement skipped"
        );
        assert!(
            sm.get("client_context").is_none(),
            "absent client_context skipped"
        );
        assert!(sm.get("idempotency_key").is_none(), "absent key skipped");
        assert!(sm.get("turn_id").is_none(), "absent turn_id skipped");
        assert!(
            sm.get("traceparent").is_none(),
            "absent traceparent skipped"
        );
    }

    // --- adele-web-ui trace propagation: turn_id / traceparent are optional in
    // both directions, so an older SPA build (whose SendMessage JSON carries
    // neither key) still parses against this newer wire type. -----------------

    #[test]
    fn an_old_shaped_send_message_still_parses() {
        // No `turn_id` or `traceparent` key at all — the shape an SPA built
        // before this change would have sent. `#[serde(default)]` on both
        // fields must fill them in as `None` rather than fail the parse, so a
        // pre-upgrade browser tab talking to a rebuilt BFF still sends.
        let json =
            r#"{"id":"send-1","command":{"send_message":{"conversation_id":"c1","content":"hi"}}}"#;
        let req: WsRequest =
            serde_json::from_str(json).expect("an old-shaped SendMessage payload must still parse");
        match req.command {
            Command::SendMessage {
                turn_id,
                traceparent,
                ..
            } => {
                assert_eq!(turn_id, None, "a missing turn_id key must default to None");
                assert_eq!(
                    traceparent, None,
                    "a missing traceparent key must default to None"
                );
            }
            other => panic!("expected SendMessage, got {other:?}"),
        }
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
            capability_change: None,
        };
        assert!(matches!(
            event_to_ui_message(ev),
            Some(UiMessage::AssistantStatus { request_id, message })
                if request_id == "r1" && message == "searching"
        ));
    }

    /// The daemon may attach a structured capability change to a status event.
    /// The shared reducer has no counterpart field yet, so the mapping keeps the
    /// status text and drops the extra data instead of failing.
    ///
    /// The reason and the tiers are read back as typed values before the
    /// mapping runs. Both types fall back to a catch-all variant for a string
    /// they do not know, so only an equality check on the decoded value proves
    /// the fixture carries the wire strings the daemon actually sends.
    #[test]
    fn assistant_status_with_a_capability_change_still_maps_to_status() {
        let frame: WsFrame = serde_json::from_str(
            r#"{"event":{"event":{"assistant_status":{"conversation_id":"c1","request_id":"r1","message":"tools that act are now refused","capability_change":{"reason":"external_content_ingested","closed_tool_tiers":["mutate","network_egress","code_execution"]}}}}}"#,
        )
        .expect("a status frame with a capability change parses");
        let WsFrame::Event { event } = frame else {
            panic!("expected an event frame");
        };
        let Event::AssistantStatus {
            capability_change: Some(change),
            ..
        } = &event
        else {
            panic!("expected a status event carrying a capability change, got {event:?}");
        };
        assert_eq!(change.reason, TurnCapabilityReason::ExternalContentIngested);
        assert_eq!(
            change.closed_tool_tiers,
            [ToolTier::Mutate, ToolTier::Egress, ToolTier::Execution]
        );
        assert!(matches!(
            event_to_ui_message(event),
            Some(UiMessage::AssistantStatus { request_id, message })
                if request_id == "r1" && message == "tools that act are now refused"
        ));
    }

    #[test]
    fn user_message_added_preserves_all_fields() {
        let ev = Event::UserMessageAdded {
            conversation_id: "c1".to_string(),
            request_id: "r1".to_string(),
            content: "hi".to_string(),
            idempotency_key: Some("turn-key".to_string()),
        };
        assert!(matches!(
            event_to_ui_message(ev),
            Some(UiMessage::UserMessageAdded { conversation_id, request_id, content, idempotency_key })
                if conversation_id == "c1"
                    && request_id == "r1"
                    && content == "hi"
                    // The echoed initiating key (#570) must survive the mapping so
                    // the initiator can dedupe its optimistic bubble by exact match.
                    && idempotency_key.as_deref() == Some("turn-key")
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
                idempotency_key: None,
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
                capability_change: None,
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
    fn knowledge_changed_maps_through() {
        // Issue #39: the KB browser live-refreshes on this. The reducer returns
        // no effects for it (the KB panel is a self-contained widget wired at the
        // window layer), but it must still map to the `UiMessage` rather than
        // being dropped by the `_` arm — dropping it is what kept the panel from
        // live-updating.
        assert!(matches!(
            event_to_ui_message(Event::KnowledgeChanged),
            Some(UiMessage::KnowledgeChanged)
        ));
    }

    // --- Background tasks (issue #50) ----------------------------------------

    fn sample_task(id: &str) -> desktop_assistant_api_model::TaskView {
        use desktop_assistant_api_model::{TaskId, TaskKind, TaskStatus, TaskView};
        TaskView {
            id: TaskId(id.to_string()),
            kind: TaskKind::Standalone {
                name: "agent".to_string(),
                conversation_id: "c1".to_string(),
            },
            status: TaskStatus::Running,
            started_at: 1_700_000_000_000,
            ended_at: None,
            last_error: None,
            parent: None,
            children: vec![],
            title: "Research".to_string(),
            progress_hint: None,
            owner_todo: String::new(),
            spawn_marker: None,
        }
    }

    #[test]
    fn task_started_maps_to_ui_task_started() {
        let ev = Event::TaskStarted {
            task: sample_task("t1"),
        };
        assert!(matches!(
            event_to_ui_message(ev),
            Some(UiMessage::TaskStarted(task)) if task.id.0 == "t1" && task.title == "Research"
        ));
    }

    #[test]
    fn task_progress_maps_to_ui_task_progress() {
        let ev = Event::TaskProgress {
            id: "t1".to_string(),
            progress_hint: Some("step 2/4".to_string()),
        };
        assert!(matches!(
            event_to_ui_message(ev),
            Some(UiMessage::TaskProgress { id, progress_hint })
                if id == "t1" && progress_hint.as_deref() == Some("step 2/4")
        ));
    }

    #[test]
    fn task_completed_maps_to_ui_task_completed_dropping_status() {
        // The reducer's `UiMessage::TaskCompleted` carries only the id; the
        // terminal status is reflected by the engine's authoritative re-fetch.
        let ev = Event::TaskCompleted {
            id: "t1".to_string(),
            status: desktop_assistant_api_model::TaskStatus::Failed,
            last_error: Some("boom".to_string()),
        };
        assert!(matches!(
            event_to_ui_message(ev),
            Some(UiMessage::TaskCompleted { id }) if id == "t1"
        ));
    }

    #[test]
    fn task_lifecycle_event_set_all_map() {
        // Pin the lifecycle set the tasks panel (#50) surfaces so a reshape can't
        // let one silently fall through the `_` arm to `None`.
        let events = [
            Event::TaskStarted {
                task: sample_task("t1"),
            },
            Event::TaskProgress {
                id: "t1".to_string(),
                progress_hint: None,
            },
            Event::TaskCompleted {
                id: "t1".to_string(),
                status: desktop_assistant_api_model::TaskStatus::Completed,
                last_error: None,
            },
        ];
        for event in events {
            let label = format!("{event:?}");
            assert!(
                event_to_ui_message(event).is_some(),
                "tasks-panel lifecycle event must map to a UiMessage: {label}"
            );
        }
    }

    #[test]
    fn unsurfaced_event_maps_to_none() {
        // `TaskLogAppended` has no web screen (the tasks panel shows status/
        // progress, not per-task logs), so it still drops through the `_` arm to
        // `None` — and the BFF relay likewise never ships log payloads.
        assert!(
            event_to_ui_message(Event::TaskLogAppended {
                id: "t1".to_string(),
                entry: desktop_assistant_api_model::TaskLogEntry {
                    seq: 1,
                    timestamp: 0,
                    level: desktop_assistant_api_model::LogLevel::Info,
                    category: desktop_assistant_api_model::LogCategory::Status,
                    message: "hi".to_string(),
                    data: None,
                },
            })
            .is_none()
        );
    }

    /// The daemon may add optional fields to the error frame; an older payload
    /// without them must still parse, and a newer one must not break the read.
    #[test]
    fn error_frame_with_a_classification_still_correlates_by_id() {
        let frame: WsFrame = serde_json::from_str(
            r#"{"error":{"id":"req-9","error":"not authorized: 'set_api_key' requires the administrator capability","detail":{"code":"not_authorized","description":"d","message":"Only a daemon administrator can do that.","retryable":false}}}"#,
        )
        .expect("a classified error frame parses");
        match frame {
            WsFrame::Error { id, error, .. } => {
                assert_eq!(id, "req-9");
                assert!(error.starts_with("not authorized:"), "{error}");
            }
            other => panic!("expected an error frame, got {other:?}"),
        }
    }

    #[test]
    fn error_frame_correlates_by_id() {
        let frame: WsFrame =
            serde_json::from_str(r#"{"error":{"id":"req-9","error":"conversation not found"}}"#)
                .expect("error frame parses");
        match frame {
            WsFrame::Error { id, error, .. } => {
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
