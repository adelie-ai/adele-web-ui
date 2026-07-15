//! BFF event-relay: fan the daemon's cross-client / background `SignalEvent`s
//! out to the connected browser sessions (issue #33), completing live
//! multi-client sync (#15) and enabling live scratchpad push (#16).
//!
//! ## The gap this closes
//! [`crate::forward::ForwardingHandler`] only projects a browser's OWN in-flight
//! turn — the events it spawns a per-request drain for, correlated by that send's
//! `daemon_request_id`. But the daemon ALSO fans cross-client / background events
//! onto the BFF's single [`Connector`] `subscribe()` stream: another client's
//! turn, a `ConversationTitleChanged`, a `ConversationListChanged`, a
//! `ScratchpadChanged`. Nothing relayed those onward, so live sync and live
//! scratchpad never worked end-to-end. This module is that relay.
//!
//! ## Design — reuse the daemon's own #1 machinery
//! The BFF embeds the daemon's `ws-interface` server as its browser front door.
//! That server already has a sanctioned way to push *server-initiated* events to
//! a browser socket: the [`AssistantApiHandler::conversation_subscriptions`]
//! hook. When the handler returns a [`ConversationSubscriptions`], the dispatcher
//! registers every browser connection's outbound sink in it at connect and
//! records which conversations each is viewing from the SPA's
//! `SubscribeConversations` (already sent on connect + reconnect). We drive that
//! same registry from a background task that drains the daemon `Connector`'s
//! signal stream and [`ConversationSubscriptions::route`]s each event to the
//! sessions viewing its conversation — exactly how the daemon fans a turn to
//! other clients, mirrored one hop out.
//!
//! ## SCOPE — v1 is single-tenant; per-user demux DEFERRED (#20/#33)
//! The web UI is single-tenant: every browser session authenticates the BFF's ONE
//! shared daemon connection as the SAME user — the configured `login_username`,
//! which `auth.rs` stamps as each session token's `sub`. So there is no per-user
//! routing to do; delivering the single user's events to that user's sessions IS
//! correct today, and route()'s user-scope (#432) is satisfied by passing that
//! one user. **Per-session / per-user demultiplexing is deliberately NOT built
//! here** — it only becomes necessary once multi-tenancy (#20) gives each browser
//! its own daemon identity. Until then this stays a broadcast-to-the-one-user.
//!
//! We DO respect `SubscribeConversations`: `route` delivers an event only to the
//! sessions viewing its conversation. A session viewing conversation `c1` gets
//! `c1`'s turn/title/scratchpad events live; a list change for a *different*
//! conversation it isn't viewing is not pushed to it in v1 (it refreshes on its
//! next interaction). Broadcasting sidebar-level changes to every session
//! regardless of what it's viewing needs the user-level event seam and rides the
//! same multi-tenant identity work (#20/#33).
//!
//! ## User-scoped events (#39)
//! `KnowledgeChanged` is not about any one conversation — the user's long-term
//! knowledge base changed — so there is no conversation to `route` on. It is
//! delivered by `ConversationSubscriptions::broadcast_to_user` to ALL of the
//! single-tenant user's sessions (whatever each is viewing), so an open KB panel
//! live-refreshes. This is the SAME single-tenant posture as the rest of the
//! relay: it broadcasts to the one user, and true per-user demux awaits
//! multi-tenancy (#20). The #432 user boundary is still enforced end to end.
//!
//! ## Trust boundary
//! The relay forwards only the daemon's own `api::Event`s (never daemon internals
//! or secrets), and route()'s per-user scope (#432) is preserved end to end — a
//! session is never delivered another user's events. Delivering a browser its OWN
//! in-flight turn again here is harmless: the reducer claims a turn's stream by
//! the first `request_id` it sees and drops frames for any other id, and the
//! per-turn path (`forward.rs`) and this relay carry the same turn under the
//! browser vs. daemon id — so a turn never double-renders.

use std::sync::Arc;

use desktop_assistant_api_model as api;
use desktop_assistant_application::conversation_subs::ConversationSubscriptions;
use desktop_assistant_client_common::{Connector, SignalEvent};

/// Synthetic "origin session" for relayed events. Real dispatcher session ids are
/// `sess-N` (see `transport-dispatch::mint_session_id`), so this matches none of
/// them: [`ConversationSubscriptions::route`] excludes only its `origin_session`,
/// and excluding a non-existent session means the event reaches EVERY session
/// viewing the conversation (the relay has no single "originating" browser to
/// suppress — the per-turn path already owns the initiator's own delivery).
const RELAY_ORIGIN_SESSION: &str = "__bff_relay__";

/// Project a daemon [`SignalEvent`] onto the browser [`api::Event`] to relay.
///
/// The inverse of `api-model`'s `map_event_to_signal`, WITHOUT the id rewrite
/// that [`crate::forward::project_turn_event`] does: a relayed event is not the
/// receiving browser's own send, so it keeps the daemon's `request_id` (the
/// reducer routes a not-initiated turn by that stable id).
///
/// The background-task lifecycle (`TaskStarted` / `TaskProgress` /
/// `TaskCompleted`) DOES map now (the tasks panel, #50). Like `KnowledgeChanged`
/// (#39) these carry no conversation, so [`relay_signal`] broadcasts them
/// user-scoped rather than routing by conversation. `TaskLogAppended` stays
/// unsurfaced — the panel shows status/progress, not per-task logs, so its
/// (potentially large) log payloads are never shipped to the browser.
///
/// Returns `None` for the remaining unsurfaced signals — `TaskLogAppended`
/// (above) and `ClientToolCall` (the web client is not an MCP host) — and for
/// the `Disconnected` control signal.
pub fn relay_signal_to_event(signal: &SignalEvent) -> Option<api::Event> {
    match signal {
        SignalEvent::UserMessageAdded {
            conversation_id,
            request_id,
            content,
        } => Some(api::Event::UserMessageAdded {
            conversation_id: conversation_id.clone(),
            request_id: request_id.clone(),
            content: content.clone(),
        }),
        SignalEvent::Chunk {
            conversation_id,
            request_id,
            chunk,
        } => Some(api::Event::AssistantDelta {
            conversation_id: conversation_id.clone(),
            request_id: request_id.clone(),
            chunk: chunk.clone(),
        }),
        SignalEvent::Complete {
            conversation_id,
            request_id,
            full_response,
        } => Some(api::Event::AssistantCompleted {
            conversation_id: conversation_id.clone(),
            request_id: request_id.clone(),
            full_response: full_response.clone(),
        }),
        SignalEvent::Error {
            conversation_id,
            request_id,
            error,
        } => Some(api::Event::AssistantError {
            conversation_id: conversation_id.clone(),
            request_id: request_id.clone(),
            error: error.clone(),
        }),
        SignalEvent::Status {
            conversation_id,
            request_id,
            message,
        } => Some(api::Event::AssistantStatus {
            conversation_id: conversation_id.clone(),
            request_id: request_id.clone(),
            message: message.clone(),
        }),
        SignalEvent::ContextUsage {
            conversation_id,
            request_id,
            used_tokens,
            budget_tokens,
            compaction_active,
        } => Some(api::Event::ContextUsage {
            conversation_id: conversation_id.clone(),
            request_id: request_id.clone(),
            used_tokens: *used_tokens,
            budget_tokens: *budget_tokens,
            compaction_active: *compaction_active,
        }),
        SignalEvent::TitleChanged {
            conversation_id,
            title,
        } => Some(api::Event::ConversationTitleChanged {
            conversation_id: conversation_id.clone(),
            title: title.clone(),
        }),
        SignalEvent::ConversationListChanged { conversation_id } => {
            Some(api::Event::ConversationListChanged {
                conversation_id: conversation_id.clone(),
            })
        }
        SignalEvent::ConversationWarning {
            conversation_id,
            warning,
        } => Some(api::Event::ConversationWarningEmitted {
            conversation_id: conversation_id.clone(),
            warning: warning.clone(),
        }),
        SignalEvent::ScratchpadChanged { conversation_id } => Some(api::Event::ScratchpadChanged {
            conversation_id: conversation_id.clone(),
        }),
        // User-scoped (#39): the user's long-term KB changed. It carries no
        // conversation, so `relay_signal` broadcasts it to all of the user's
        // sessions rather than routing by conversation.
        SignalEvent::KnowledgeChanged => Some(api::Event::KnowledgeChanged),
        // User-scoped (#50): the background-task lifecycle. Like KnowledgeChanged
        // these carry no conversation, so `relay_signal` broadcasts them to all
        // of the user's sessions so an open tasks panel live-updates.
        SignalEvent::TaskStarted { task } => Some(api::Event::TaskStarted { task: task.clone() }),
        SignalEvent::TaskProgress { id, progress_hint } => Some(api::Event::TaskProgress {
            id: id.clone(),
            progress_hint: progress_hint.clone(),
        }),
        SignalEvent::TaskCompleted {
            id,
            status,
            last_error,
        } => Some(api::Event::TaskCompleted {
            id: id.clone(),
            status: *status,
            last_error: last_error.clone(),
        }),
        // Not surfaced by the web UI: `TaskLogAppended` (the panel shows status/
        // progress, not per-task logs, so log payloads are never shipped) and
        // `ClientToolCall` (not an MCP host); `Disconnected` is a control signal
        // handled by the loop.
        SignalEvent::TaskLogAppended { .. }
        | SignalEvent::ClientToolCall { .. }
        | SignalEvent::Disconnected { .. } => None,
    }
}

/// The conversation an [`api::Event`] belongs to, used to route it to the
/// sessions viewing that conversation. Every variant [`relay_signal_to_event`]
/// emits carries one; anything else is not relayed.
fn event_conversation_id(event: &api::Event) -> Option<&str> {
    match event {
        api::Event::UserMessageAdded {
            conversation_id, ..
        }
        | api::Event::AssistantDelta {
            conversation_id, ..
        }
        | api::Event::AssistantCompleted {
            conversation_id, ..
        }
        | api::Event::AssistantError {
            conversation_id, ..
        }
        | api::Event::AssistantStatus {
            conversation_id, ..
        }
        | api::Event::ContextUsage {
            conversation_id, ..
        }
        | api::Event::ConversationTitleChanged {
            conversation_id, ..
        }
        | api::Event::ConversationListChanged { conversation_id }
        | api::Event::ConversationWarningEmitted {
            conversation_id, ..
        }
        | api::Event::ScratchpadChanged { conversation_id } => Some(conversation_id),
        _ => None,
    }
}

/// Relay one daemon signal to the browser sessions that should see it.
///
/// A conversation-scoped event is routed to the sessions viewing its
/// conversation; a **user-scoped** event (one that maps but carries no
/// conversation — `KnowledgeChanged`, #39) is broadcast to ALL of the
/// (single-tenant) user's sessions, whatever each is viewing, so an open KB
/// panel live-refreshes. Both paths honour route()/broadcast()'s #432 user
/// boundary — a session is never delivered another user's event.
///
/// Returns `true` when the signal mapped to a relayable event and was delivered,
/// `false` when it was dropped (not surfaced by the web UI). Never panics on any
/// variant — a `Disconnected` simply maps to `None` and is dropped here (the
/// loop in [`run_relay`] logs it).
async fn relay_signal(
    signal: &SignalEvent,
    subs: &ConversationSubscriptions,
    origin_user: &str,
) -> bool {
    let Some(event) = relay_signal_to_event(signal) else {
        return false;
    };
    match event_conversation_id(&event) {
        Some(conversation_id) => {
            subs.route(conversation_id, &event, RELAY_ORIGIN_SESSION, origin_user)
                .await;
        }
        // No conversation to route on ⇒ user-scoped (the only such relayed event
        // is `KnowledgeChanged`). Fan it to every one of the user's sessions.
        None => subs.broadcast_to_user(&event, origin_user).await,
    }
    true
}

/// Drain the daemon `Connector`'s signal stream and relay each event to the
/// browser sessions viewing its conversation, for the process lifetime.
///
/// `origin_user` is the single-tenant user every browser session authenticates
/// as (the BFF's `login_username`); it satisfies route()'s per-user scope (#432).
/// A `Disconnected` signal is logged and skipped — the `Connector` reconnects in
/// place and keeps feeding this same stream (#246), so the relay resumes without
/// re-subscribing. The loop ends only when the stream closes (the `Connector` is
/// gone / the process is shutting down).
pub async fn run_relay(
    connector: Arc<Connector>,
    subs: Arc<ConversationSubscriptions>,
    origin_user: String,
) {
    let mut rx = connector.subscribe();
    tracing::info!("BFF event-relay started");
    while let Some(signal) = rx.recv().await {
        if let SignalEvent::Disconnected { reason } = &signal {
            tracing::warn!(%reason, "daemon signal stream disconnected; relay awaiting reconnect");
            continue;
        }
        relay_signal(&signal, &subs, &origin_user).await;
    }
    tracing::info!("daemon signal stream ended; BFF event-relay stopping");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use desktop_assistant_api_model::{ConversationModelSelectionView, ConversationWarning};
    use desktop_assistant_application::EventSink;

    const USER: &str = "adele";
    const OTHER_USER: &str = "mallory";
    const DAEMON_RID: &str = "daemon-req-7";

    /// Records the events emitted to a registered session so a test can assert
    /// what that browser connection received through the relay.
    #[derive(Default)]
    struct RecordingSink(Mutex<Vec<api::Event>>);

    #[async_trait::async_trait]
    impl EventSink for RecordingSink {
        async fn emit(&self, event: api::Event) -> bool {
            self.0.lock().expect("sink lock").push(event);
            true
        }
    }

    fn chunk(conversation_id: &str) -> SignalEvent {
        SignalEvent::Chunk {
            conversation_id: conversation_id.to_string(),
            request_id: DAEMON_RID.to_string(),
            chunk: "hi".to_string(),
        }
    }

    // --- SignalEvent -> api::Event mapping (one test per relayed variant) -----

    #[test]
    fn user_message_added_maps_preserving_ids_and_content() {
        let ev = relay_signal_to_event(&SignalEvent::UserMessageAdded {
            conversation_id: "c1".into(),
            request_id: DAEMON_RID.into(),
            content: "hello".into(),
        })
        .expect("relayed");
        assert!(matches!(
            ev,
            api::Event::UserMessageAdded { conversation_id, request_id, content }
                if conversation_id == "c1" && request_id == DAEMON_RID && content == "hello"
        ));
    }

    #[test]
    fn chunk_maps_to_assistant_delta_keeping_daemon_request_id() {
        // A relayed turn is NOT the receiving browser's own send: it keeps the
        // daemon's request_id (no rewrite, unlike the per-turn path).
        let ev = relay_signal_to_event(&chunk("c1")).expect("relayed");
        assert!(matches!(
            ev,
            api::Event::AssistantDelta { conversation_id, request_id, chunk }
                if conversation_id == "c1" && request_id == DAEMON_RID && chunk == "hi"
        ));
    }

    #[test]
    fn complete_maps_to_assistant_completed() {
        let ev = relay_signal_to_event(&SignalEvent::Complete {
            conversation_id: "c1".into(),
            request_id: DAEMON_RID.into(),
            full_response: "done".into(),
        })
        .expect("relayed");
        assert!(matches!(
            ev,
            api::Event::AssistantCompleted { full_response, request_id, .. }
                if full_response == "done" && request_id == DAEMON_RID
        ));
    }

    #[test]
    fn error_maps_to_assistant_error() {
        let ev = relay_signal_to_event(&SignalEvent::Error {
            conversation_id: "c1".into(),
            request_id: DAEMON_RID.into(),
            error: "boom".into(),
        })
        .expect("relayed");
        assert!(matches!(ev, api::Event::AssistantError { error, .. } if error == "boom"));
    }

    #[test]
    fn status_maps_to_assistant_status() {
        let ev = relay_signal_to_event(&SignalEvent::Status {
            conversation_id: "c1".into(),
            request_id: DAEMON_RID.into(),
            message: "searching".into(),
        })
        .expect("relayed");
        assert!(
            matches!(ev, api::Event::AssistantStatus { message, .. } if message == "searching")
        );
    }

    #[test]
    fn context_usage_maps_preserving_counts() {
        let ev = relay_signal_to_event(&SignalEvent::ContextUsage {
            conversation_id: "c1".into(),
            request_id: DAEMON_RID.into(),
            used_tokens: 10,
            budget_tokens: 100,
            compaction_active: true,
        })
        .expect("relayed");
        assert!(matches!(
            ev,
            api::Event::ContextUsage { used_tokens, budget_tokens, compaction_active, .. }
                if used_tokens == 10 && budget_tokens == 100 && compaction_active
        ));
    }

    #[test]
    fn title_changed_maps_to_conversation_title_changed() {
        let ev = relay_signal_to_event(&SignalEvent::TitleChanged {
            conversation_id: "c1".into(),
            title: "Renamed".into(),
        })
        .expect("relayed");
        assert!(matches!(
            ev,
            api::Event::ConversationTitleChanged { conversation_id, title }
                if conversation_id == "c1" && title == "Renamed"
        ));
    }

    #[test]
    fn conversation_list_changed_maps_through() {
        let ev = relay_signal_to_event(&SignalEvent::ConversationListChanged {
            conversation_id: "c2".into(),
        })
        .expect("relayed");
        assert!(matches!(
            ev,
            api::Event::ConversationListChanged { conversation_id } if conversation_id == "c2"
        ));
    }

    #[test]
    fn conversation_warning_maps_to_warning_emitted() {
        let selection = |conn: &str, model: &str| ConversationModelSelectionView {
            connection_id: conn.to_string(),
            model_id: model.to_string(),
            effort: None,
        };
        let ev = relay_signal_to_event(&SignalEvent::ConversationWarning {
            conversation_id: "c1".into(),
            warning: ConversationWarning::DanglingModelSelection {
                previous_selection: selection("gone", "ghost"),
                fallback_to: selection("openai", "gpt-4o"),
            },
        })
        .expect("relayed");
        assert!(matches!(
            ev,
            api::Event::ConversationWarningEmitted {
                conversation_id,
                warning: ConversationWarning::DanglingModelSelection { .. }
            } if conversation_id == "c1"
        ));
    }

    #[test]
    fn scratchpad_changed_maps_through() {
        // Issue #16: the live-push path the web scratchpad view is waiting for.
        let ev = relay_signal_to_event(&SignalEvent::ScratchpadChanged {
            conversation_id: "c1".into(),
        })
        .expect("relayed");
        assert!(matches!(
            ev,
            api::Event::ScratchpadChanged { conversation_id } if conversation_id == "c1"
        ));
    }

    #[test]
    fn knowledge_changed_maps_to_knowledge_event() {
        // Issue #39: the user's long-term KB changed. Unlike the conversation-
        // scoped events, this maps to a wire event that carries no conversation
        // (it is user-scoped), so `relay_signal` fans it out differently.
        let ev = relay_signal_to_event(&SignalEvent::KnowledgeChanged).expect("relayed");
        assert!(matches!(ev, api::Event::KnowledgeChanged));
    }

    #[test]
    fn unsurfaced_and_control_signals_are_not_relayed() {
        // `TaskLogAppended` is unsurfaced (the tasks panel shows status/progress,
        // not per-task logs — #50), the web UI is not an MCP host, and
        // `Disconnected` is a control signal — none map to a relayable event.
        // (`KnowledgeChanged` and the Task* lifecycle ARE relayed now,
        // user-scoped — see their own tests.)
        for signal in [
            SignalEvent::TaskLogAppended {
                id: "t1".into(),
                entry: api::TaskLogEntry {
                    seq: 1,
                    timestamp: 0,
                    level: api::LogLevel::Info,
                    category: api::LogCategory::Status,
                    message: "hi".into(),
                    data: None,
                },
            },
            SignalEvent::ClientToolCall {
                task_id: "t1".into(),
                conversation_id: "c1".into(),
                tool_call_id: "tc1".into(),
                tool_name: "shell".into(),
                arguments: serde_json::Value::Null,
            },
            SignalEvent::Disconnected {
                reason: "socket closed".into(),
            },
        ] {
            let label = format!("{signal:?}");
            assert!(
                relay_signal_to_event(&signal).is_none(),
                "must not relay {label}"
            );
        }
    }

    #[test]
    fn every_relayed_event_has_a_conversation_to_route_on() {
        // The routing invariant for CONVERSATION-scoped events: each MUST carry a
        // conversation id so `relay_signal` routes rather than drops it. Pin the
        // whole conversation-scoped set so a new such variant can't forget one.
        // (The user-scoped exceptions — `KnowledgeChanged` and the Task*
        // lifecycle — deliberately carry no conversation and are broadcast
        // instead; see their own tests.)
        let relayed = [
            SignalEvent::UserMessageAdded {
                conversation_id: "c1".into(),
                request_id: DAEMON_RID.into(),
                content: "hi".into(),
            },
            chunk("c1"),
            SignalEvent::Complete {
                conversation_id: "c1".into(),
                request_id: DAEMON_RID.into(),
                full_response: "done".into(),
            },
            SignalEvent::Error {
                conversation_id: "c1".into(),
                request_id: DAEMON_RID.into(),
                error: "boom".into(),
            },
            SignalEvent::Status {
                conversation_id: "c1".into(),
                request_id: DAEMON_RID.into(),
                message: "searching".into(),
            },
            SignalEvent::ContextUsage {
                conversation_id: "c1".into(),
                request_id: DAEMON_RID.into(),
                used_tokens: 1,
                budget_tokens: 2,
                compaction_active: false,
            },
            SignalEvent::TitleChanged {
                conversation_id: "c1".into(),
                title: "T".into(),
            },
            SignalEvent::ConversationListChanged {
                conversation_id: "c1".into(),
            },
            SignalEvent::ScratchpadChanged {
                conversation_id: "c1".into(),
            },
        ];
        for signal in relayed {
            let label = format!("{signal:?}");
            let event =
                relay_signal_to_event(&signal).unwrap_or_else(|| panic!("must relay {label}"));
            assert!(
                event_conversation_id(&event).is_some(),
                "relayed event must carry a conversation to route on: {label}"
            );
        }
    }

    // --- Routing / broadcast behaviour ----------------------------------------

    fn viewer(
        subs: &ConversationSubscriptions,
        session: &str,
        user: &str,
        convs: &[&str],
    ) -> Arc<RecordingSink> {
        let sink = Arc::new(RecordingSink::default());
        subs.register(session, user, sink.clone());
        subs.set_subscriptions(session, convs.iter().map(|c| c.to_string()).collect());
        sink
    }

    #[tokio::test]
    async fn broadcast_reaches_every_session_viewing_the_conversation() {
        // Two browser sessions of the one (single-tenant) user, both viewing c1:
        // a relayed turn event must reach BOTH — this is cross-client live sync.
        let subs = ConversationSubscriptions::new();
        let a = viewer(&subs, "sess-1", USER, &["c1"]);
        let b = viewer(&subs, "sess-2", USER, &["c1"]);

        assert!(relay_signal(&chunk("c1"), &subs, USER).await, "routed");

        for (name, sink) in [("A", &a), ("B", &b)] {
            let got = sink.0.lock().unwrap();
            assert_eq!(
                got.len(),
                1,
                "session {name} must receive the relayed event"
            );
            assert!(
                matches!(&got[0], api::Event::AssistantDelta { request_id, .. } if request_id == DAEMON_RID),
                "session {name} keeps the daemon request id, got {:?}",
                got[0]
            );
        }
    }

    #[tokio::test]
    async fn does_not_reach_a_session_viewing_a_different_conversation() {
        // Respect SubscribeConversations: a session viewing c2 must not receive
        // c1's events.
        let subs = ConversationSubscriptions::new();
        let elsewhere = viewer(&subs, "sess-1", USER, &["c2"]);

        assert!(relay_signal(&chunk("c1"), &subs, USER).await, "routed");

        assert!(
            elsewhere.0.lock().unwrap().is_empty(),
            "a session viewing c2 must not get c1's turn events"
        );
    }

    #[tokio::test]
    async fn does_not_cross_the_user_boundary() {
        // Trust boundary (#432): even subscribed to c1, a DIFFERENT user's
        // session is never delivered the relay's (single) user's events.
        let subs = ConversationSubscriptions::new();
        let intruder = viewer(&subs, "sess-evil", OTHER_USER, &["c1"]);

        assert!(relay_signal(&chunk("c1"), &subs, USER).await, "routed");

        assert!(
            intruder.0.lock().unwrap().is_empty(),
            "another user's session must never receive relayed events"
        );
    }

    #[tokio::test]
    async fn scratchpad_change_is_relayed_to_viewers() {
        // Issue #16 end-to-end at the relay: a background ScratchpadChanged is
        // pushed to the session viewing that conversation.
        let subs = ConversationSubscriptions::new();
        let viewing = viewer(&subs, "sess-1", USER, &["c1"]);

        let signal = SignalEvent::ScratchpadChanged {
            conversation_id: "c1".into(),
        };
        assert!(relay_signal(&signal, &subs, USER).await, "routed");

        let got = viewing.0.lock().unwrap();
        assert!(
            matches!(got.as_slice(), [api::Event::ScratchpadChanged { conversation_id }] if conversation_id == "c1"),
            "the scratchpad-change must reach the viewing session, got {got:?}"
        );
    }

    #[tokio::test]
    async fn knowledge_changed_is_broadcast_to_the_user_regardless_of_subscription() {
        // Issue #39: `KnowledgeChanged` is user-scoped — it carries no
        // conversation to route on. It must reach EVERY one of the user's
        // sessions whatever each is viewing, so an open KB panel live-refreshes.
        // Here two sessions view *different* conversations (and a third views
        // none); all three must still receive it.
        let subs = ConversationSubscriptions::new();
        let a = viewer(&subs, "sess-1", USER, &["c1"]);
        let b = viewer(&subs, "sess-2", USER, &["c2"]);
        let c = viewer(&subs, "sess-3", USER, &[]); // subscribed to nothing

        assert!(
            relay_signal(&SignalEvent::KnowledgeChanged, &subs, USER).await,
            "KnowledgeChanged must be relayed (routed), not dropped"
        );

        for (name, sink) in [("A", &a), ("B", &b), ("C", &c)] {
            let got = sink.0.lock().unwrap();
            assert!(
                matches!(got.as_slice(), [api::Event::KnowledgeChanged]),
                "session {name} must receive the user-scoped KnowledgeChanged, got {got:?}"
            );
        }
    }

    #[tokio::test]
    async fn knowledge_changed_broadcast_does_not_cross_the_user_boundary() {
        // Trust boundary (#432): a DIFFERENT user's session — even subscribed to
        // the same conversation ids — is never delivered the broadcast.
        let subs = ConversationSubscriptions::new();
        let intruder = viewer(&subs, "sess-evil", OTHER_USER, &["c1"]);

        assert!(
            relay_signal(&SignalEvent::KnowledgeChanged, &subs, USER).await,
            "routed"
        );

        assert!(
            intruder.0.lock().unwrap().is_empty(),
            "another user's session must never receive the KB broadcast"
        );
    }

    // --- Background tasks (issue #50) -----------------------------------------

    fn sample_task(id: &str) -> api::TaskView {
        api::TaskView {
            id: api::TaskId(id.to_string()),
            kind: api::TaskKind::Standalone {
                name: "agent".to_string(),
                conversation_id: "c1".to_string(),
            },
            status: api::TaskStatus::Running,
            started_at: 1_700_000_000_000,
            ended_at: None,
            last_error: None,
            parent: None,
            children: vec![],
            title: "Research".to_string(),
            progress_hint: None,
        }
    }

    #[test]
    fn task_started_maps_preserving_the_task_view() {
        let ev = relay_signal_to_event(&SignalEvent::TaskStarted {
            task: sample_task("t1"),
        })
        .expect("TaskStarted must relay to the tasks panel (#50)");
        assert!(matches!(
            ev,
            api::Event::TaskStarted { task } if task.id.0 == "t1" && task.title == "Research"
        ));
    }

    #[test]
    fn task_progress_maps_preserving_id_and_hint() {
        let ev = relay_signal_to_event(&SignalEvent::TaskProgress {
            id: "t1".into(),
            progress_hint: Some("step 2/4".into()),
        })
        .expect("TaskProgress must relay (#50)");
        assert!(matches!(
            ev,
            api::Event::TaskProgress { id, progress_hint }
                if id == "t1" && progress_hint.as_deref() == Some("step 2/4")
        ));
    }

    #[test]
    fn task_completed_maps_preserving_terminal_status() {
        let ev = relay_signal_to_event(&SignalEvent::TaskCompleted {
            id: "t1".into(),
            status: api::TaskStatus::Failed,
            last_error: Some("boom".into()),
        })
        .expect("TaskCompleted must relay (#50)");
        assert!(matches!(
            ev,
            api::Event::TaskCompleted { id, status, last_error }
                if id == "t1" && status == api::TaskStatus::Failed && last_error.as_deref() == Some("boom")
        ));
    }

    #[tokio::test]
    async fn task_events_broadcast_to_every_user_session_regardless_of_subscription() {
        // Task events are user-scoped (they carry no conversation to route on),
        // so — like `KnowledgeChanged` (#39) — they must reach EVERY one of the
        // user's sessions whatever each is viewing, so an open tasks panel
        // live-updates.
        let subs = ConversationSubscriptions::new();
        let a = viewer(&subs, "sess-1", USER, &["c1"]);
        let b = viewer(&subs, "sess-2", USER, &["c2"]);
        let c = viewer(&subs, "sess-3", USER, &[]); // subscribed to nothing

        let signal = SignalEvent::TaskProgress {
            id: "t1".into(),
            progress_hint: Some("step 2/4".into()),
        };
        assert!(
            relay_signal(&signal, &subs, USER).await,
            "TaskProgress must be relayed (broadcast), not dropped"
        );

        for (name, sink) in [("A", &a), ("B", &b), ("C", &c)] {
            let got = sink.0.lock().unwrap();
            assert!(
                matches!(got.as_slice(), [api::Event::TaskProgress { id, .. }] if id == "t1"),
                "session {name} must receive the user-scoped task event, got {got:?}"
            );
        }
    }

    #[tokio::test]
    async fn task_broadcast_does_not_cross_the_user_boundary() {
        // Trust boundary (#432): a DIFFERENT user's session — even subscribed to
        // the same conversation ids — is never delivered another user's task
        // events.
        let subs = ConversationSubscriptions::new();
        let intruder = viewer(&subs, "sess-evil", OTHER_USER, &["c1"]);

        assert!(
            relay_signal(
                &SignalEvent::TaskStarted {
                    task: sample_task("t1")
                },
                &subs,
                USER
            )
            .await,
            "routed"
        );

        assert!(
            intruder.0.lock().unwrap().is_empty(),
            "another user's session must never receive relayed task events"
        );
    }

    #[tokio::test]
    async fn disconnected_signal_routes_nothing_and_does_not_panic() {
        // The relay must survive a `Disconnected` on the daemon stream: it maps
        // to no event and delivers nothing (the loop logs and keeps going).
        let subs = ConversationSubscriptions::new();
        let viewing = viewer(&subs, "sess-1", USER, &["c1"]);

        let routed = relay_signal(
            &SignalEvent::Disconnected {
                reason: "socket closed".into(),
            },
            &subs,
            USER,
        )
        .await;

        assert!(!routed, "Disconnected must not route");
        assert!(
            viewing.0.lock().unwrap().is_empty(),
            "Disconnected must deliver nothing"
        );
    }
}
