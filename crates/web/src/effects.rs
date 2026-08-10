//! What the SPA does with each shared-reducer [`Effect`] (issue #73).
//!
//! Every client mirrors `client-ui-common`'s effects into its own view by hand,
//! so an effect no client arm matches is a silent no-op: the reducer decided
//! something should reach the user and nothing did. [`Disposition`] makes that
//! decision explicit — the engine's executor returns it for every effect it
//! runs, so an effect is either performed or dropped *with a stated reason*, and
//! the host coverage test can assert the reason exists.
//!
//! The enumeration itself is the compiler's job: the executor matches `Effect`
//! exhaustively with no `_` arm, so a new variant upstream is a build error
//! rather than a new silent drop.

/// What the engine's executor did with one [`Effect`](client_ui_common::Effect).
///
/// The reason string is what the coverage test reads; the wasm build only ever
/// discards it (the executor has nowhere to report to), so the field is dead
/// code there alone.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    /// Performed: mirrored into a view signal a component renders, or issued as
    /// an RPC.
    Handled,
    /// Deliberately dropped, carrying the reason it is safe to drop.
    Ignored(&'static str),
}

/// The one-line console report for a turn that ended
/// ([`Effect::TurnFinished`](client_ui_common::Effect::TurnFinished)).
///
/// The reducer reports a turn wherever it drops, clears or replaces a stream:
/// the reply completes, the turn fails, the socket drops, the conversation is
/// deleted, or a later ack replaces a turn still in flight. It reports a
/// backgrounded conversation's turn too, which is the case the SPA could not
/// see at all before.
///
/// This is the close of the correlation the send path opens. Without it a
/// person sees every turn start and no turn end.
///
/// The field is printed as `turn_id` because that is what the send line calls
/// it, and the two are meant to be the same value. That identity is a CONTRACT
/// ACROSS THREE REPOSITORIES, and no test in this one proves it end to end.
/// Each leg is held separately: `build_send_command` mints a canonical
/// non-nil v4 uuid (`send_command_carries_a_turn_id`), the daemon adopts a
/// supplied turn id as the turn's `request_id` rather than minting its own
/// (`desktop-assistant`'s `adopt_or_mint_turn_id`, and only for a value that
/// parses as a non-nil uuid), and the reducer reports the `request_id` it
/// routed the stream under (`client-ui-common`#51). So the two ids match while
/// all three hold, and a person who greps one and finds nothing should suspect
/// the middle leg first. Nothing here breaks if they ever diverge: the line
/// still reports the daemon's own id for the turn.
///
/// It carries ids only. [`TurnOutcome::Failed`](client_ui_common::TurnOutcome)
/// holds daemon- or provider-supplied text that upstream documents as untrusted
/// for telemetry — a content refusal can quote the words it refused — so the
/// outcome reaches the line as `completed` or `failed` and the text does not.
/// The user-facing message is the reducer's own `SetStatusText`, which the
/// status line already renders.
///
/// A missing value prints `-`: `request_id` is empty when a teardown ended the
/// turn before the daemon's id arrived, and `idempotency_key` is `None` for a
/// keyless send and for an external turn (voice, another client) this SPA never
/// sent.
///
/// Pure, and separate from the executor arm, because the executor needs live
/// signals and a browser console; this is what the tests below can hold to the
/// content contract.
pub fn turn_report_line(
    conversation_id: &str,
    request_id: &str,
    idempotency_key: Option<&str>,
    outcome: &client_ui_common::TurnOutcome,
) -> String {
    fn or_dash(value: &str) -> &str {
        if value.is_empty() { "-" } else { value }
    }
    let outcome = match outcome {
        client_ui_common::TurnOutcome::Completed => "completed",
        client_ui_common::TurnOutcome::Failed(_) => "failed",
    };
    format!(
        "Adele turn finished: turn_id={} conversation={} idempotency_key={} outcome={outcome}",
        or_dash(request_id),
        or_dash(conversation_id),
        or_dash(idempotency_key.unwrap_or_default()),
    )
}

#[cfg(test)]
pub mod census {
    //! One sample of every [`Effect`] variant, and the ordinal census that keeps
    //! that sample list complete.
    //!
    //! [`ordinal`] matches exhaustively, so a new upstream variant fails to
    //! compile here; [`VARIANT_COUNT`] then makes the omission of its *sample*
    //! a test failure rather than a quiet coverage hole.

    use client_ui_common::{AdeleOutput, ContextUsageView, Effect, SelectedModel, TurnOutcome};
    use desktop_assistant_api_model as api;
    use desktop_assistant_api_model::client::{
        ConversationDetail, ConversationSummary, MessageKind,
    };

    /// How many variants [`Effect`] has. Bump it when `ordinal` gains an arm.
    pub const VARIANT_COUNT: usize = 38;

    /// A stable index per [`Effect`] variant, used only to prove
    /// [`every_variant`] covers them all.
    pub fn ordinal(effect: &Effect) -> usize {
        match effect {
            Effect::ClearClient => 0,
            Effect::SetStatusText(_) => 1,
            Effect::SetSendSensitive(_) => 2,
            Effect::SetComposerText(_) => 3,
            Effect::SetQueuedMessages { .. } => 4,
            Effect::SetConversations(_) => 5,
            Effect::EnsureActiveConversation => 6,
            Effect::LoadConversationIntoChat(_) => 7,
            Effect::ReloadConversation(_) => 8,
            Effect::LoadConversation(_) => 9,
            Effect::RefetchConversationList => 10,
            Effect::ClearChat => 11,
            Effect::SetChatStatus(_) => 12,
            Effect::ClearChatStatus => 13,
            Effect::SetContextUsage(_) => 14,
            Effect::AddUserMessage(_) => 15,
            Effect::ReceiveChunk(_) => 16,
            Effect::CompleteStreaming(_) => 17,
            Effect::SendPrompt { .. } => 18,
            Effect::SetModelSelection(_) => 19,
            Effect::SetModels(_) => 20,
            Effect::SetDefaultModel(_) => 21,
            Effect::SetModelPickerVisible(_) => 22,
            Effect::ShowToast(_) => 23,
            Effect::TasksReplaceAll(_) => 24,
            Effect::TaskStarted(_) => 25,
            Effect::TaskProgress { .. } => 26,
            Effect::TaskLogAppended { .. } => 27,
            Effect::TaskCompleted { .. } => 28,
            Effect::SubscribeConversations(_) => 29,
            Effect::FetchScratchpad(_) => 30,
            Effect::SidePaneSetScratchpad(_) => 31,
            Effect::RefreshSidePaneTasks => 32,
            Effect::Speak(_) => 33,
            Effect::AddLocalMessage { .. } => 34,
            Effect::SetAdeleOutputDropdown(_) => 35,
            Effect::SubmitClientToolResult { .. } => 36,
            Effect::TurnFinished { .. } => 37,
        }
    }

    /// One instance of every [`Effect`] variant, in declaration order.
    pub fn every_variant() -> Vec<Effect> {
        vec![
            Effect::ClearClient,
            Effect::SetStatusText("Error: 429 Too Many Requests".to_string()),
            Effect::SetSendSensitive(true),
            Effect::SetComposerText("draft".to_string()),
            Effect::SetQueuedMessages {
                messages: vec!["queued".to_string()],
                editing: None,
            },
            Effect::SetConversations(vec![summary("c1")]),
            Effect::EnsureActiveConversation,
            Effect::LoadConversationIntoChat(detail("c1")),
            Effect::ReloadConversation("c1".to_string()),
            Effect::LoadConversation("c1".to_string()),
            Effect::RefetchConversationList,
            Effect::ClearChat,
            Effect::SetChatStatus("Searching knowledge base…".to_string()),
            Effect::ClearChatStatus,
            Effect::SetContextUsage(Some(ContextUsageView {
                used_tokens: 12_000,
                budget_tokens: 32_000,
                compaction_active: false,
            })),
            Effect::AddUserMessage("hi".to_string()),
            Effect::ReceiveChunk("chunk".to_string()),
            Effect::CompleteStreaming("the answer".to_string()),
            Effect::SendPrompt {
                conversation_id: "c1".to_string(),
                prompt: "hi".to_string(),
                system_refinement: None,
                idempotency_key: None,
            },
            Effect::SetModelSelection(Some(api::ConversationModelSelectionView {
                connection_id: "conn".to_string(),
                model_id: "model".to_string(),
                effort: None,
            })),
            Effect::SetModels(vec![listing()]),
            Effect::SetDefaultModel(Some(SelectedModel {
                connection_id: "conn".to_string(),
                model_id: "model".to_string(),
            })),
            Effect::SetModelPickerVisible(true),
            Effect::ShowToast("heads up".to_string()),
            Effect::TasksReplaceAll(vec![task("t1")]),
            Effect::TaskStarted(task("t1")),
            Effect::TaskProgress {
                id: "t1".to_string(),
                progress_hint: Some("step 2/4".to_string()),
            },
            Effect::TaskLogAppended {
                id: "t1".to_string(),
                entry: log_entry(),
            },
            Effect::TaskCompleted {
                id: "t1".to_string(),
            },
            Effect::SubscribeConversations(vec!["c1".to_string()]),
            Effect::FetchScratchpad("c1".to_string()),
            Effect::SidePaneSetScratchpad(vec![note("todo-1")]),
            Effect::RefreshSidePaneTasks,
            Effect::Speak("spoken aside".to_string()),
            Effect::AddLocalMessage {
                content: "spoken aside".to_string(),
                kind: MessageKind::Spoken,
            },
            Effect::SetAdeleOutputDropdown(AdeleOutput::OnDemand),
            Effect::SubmitClientToolResult {
                task_id: "t1".to_string(),
                tool_call_id: "call-1".to_string(),
                result: Ok("spoken".to_string()),
            },
            Effect::TurnFinished {
                conversation_id: "c1".to_string(),
                request_id: "11111111-2222-4333-8444-555555555555".to_string(),
                idempotency_key: Some("send-key-1".to_string()),
                outcome: TurnOutcome::Completed,
            },
        ]
    }

    fn summary(id: &str) -> ConversationSummary {
        ConversationSummary {
            id: id.to_string(),
            title: format!("Conversation {id}"),
            message_count: 0,
            archived: false,
        }
    }

    fn detail(id: &str) -> ConversationDetail {
        ConversationDetail {
            id: id.to_string(),
            title: format!("Conversation {id}"),
            messages: Vec::new(),
            model_selection: None,
            conversation_personality: None,
            tool_gate_disabled: false,
        }
    }

    fn listing() -> api::ModelListing {
        api::ModelListing {
            connection_id: "conn".to_string(),
            connection_label: "Connection (test)".to_string(),
            model: api::ModelInfoView {
                id: "model".to_string(),
                display_name: "Model".to_string(),
                context_limit: None,
                capabilities: api::ModelCapabilitiesView::default(),
            },
            notices: Vec::new(),
        }
    }

    fn task(id: &str) -> api::TaskView {
        api::TaskView {
            id: api::TaskId(id.into()),
            kind: api::TaskKind::Standalone {
                name: "agent".into(),
                conversation_id: "c1".into(),
            },
            status: api::TaskStatus::Running,
            started_at: 1_700_000_000_000,
            ended_at: None,
            last_error: None,
            parent: None,
            children: vec![],
            title: format!("Task {id}"),
            progress_hint: None,
            owner_todo: String::new(),
            spawn_marker: None,
        }
    }

    fn log_entry() -> api::TaskLogEntry {
        api::TaskLogEntry {
            seq: 1,
            timestamp: 1_700_000_000_000,
            level: api::LogLevel::Info,
            category: api::LogCategory::Status,
            message: "fetching page 2/4".to_string(),
            data: None,
        }
    }

    fn note(key: &str) -> api::ScratchpadNoteView {
        api::ScratchpadNoteView {
            id: format!("id-{key}"),
            key: key.to_string(),
            content: "write the tests first".to_string(),
            note_type: "todo".to_string(),
            sequence: None,
            done: false,
            updated_at: "2026-07-26 00:00:00".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::census::{VARIANT_COUNT, every_variant, ordinal};
    use super::turn_report_line;
    use client_ui_common::TurnOutcome;

    // --- The finished-turn report (client-ui-common#51) ----------------------
    // The SPA logs one line per send ("Adele turn_id: …") and needs the matching
    // close line, or the browser console shows a turn that starts and never ends.

    #[test]
    fn a_finished_turn_is_reported_with_its_correlation_ids() {
        let line = turn_report_line(
            "conv-1",
            "11111111-2222-4333-8444-555555555555",
            Some("send-key-9"),
            &TurnOutcome::Completed,
        );
        for id in [
            "conv-1",
            "11111111-2222-4333-8444-555555555555",
            "send-key-9",
        ] {
            assert!(
                line.contains(id),
                "the turn report must carry {id:?} so a person can pair it with the \
                 send line and with the daemon's log: {line:?}"
            );
        }
    }

    #[test]
    fn a_failed_turn_is_reported_as_failed() {
        let line = turn_report_line(
            "conv-1",
            "11111111-2222-4333-8444-555555555555",
            None,
            &TurnOutcome::Failed("the provider refused".to_string()),
        );
        assert!(
            line.contains("failed"),
            "a failed turn must say so, or the console shows every turn as a success: {line:?}"
        );
    }

    #[test]
    fn a_finished_turn_report_omits_the_failure_text() {
        // `TurnOutcome::Failed` carries daemon- or provider-supplied text that
        // upstream documents as untrusted for telemetry: a refusal can quote the
        // words it refused. The report carries ids only, so the fact of failure
        // reaches the console and the content does not.
        let line = turn_report_line(
            "conv-1",
            "11111111-2222-4333-8444-555555555555",
            None,
            &TurnOutcome::Failed("blocked: my bank account number is 12345".to_string()),
        );
        assert!(
            !line.contains("bank account"),
            "the failure text must stay off the report: {line:?}"
        );
    }

    #[test]
    fn a_keyless_send_reports_a_turn_with_no_key() {
        // A keyless send, and an external turn this client never sent, both
        // arrive with `idempotency_key: None`. The report still names the turn.
        let line = turn_report_line(
            "conv-1",
            "11111111-2222-4333-8444-555555555555",
            None,
            &TurnOutcome::Completed,
        );
        assert!(
            line.contains("11111111-2222-4333-8444-555555555555"),
            "a keyless turn is still reported by its turn id: {line:?}"
        );
    }

    #[test]
    fn a_turn_that_ended_before_its_id_arrived_is_still_reported() {
        // A teardown (a dropped socket, a deleted conversation) ends a turn
        // before the daemon's id ever reaches the client, so `request_id` is
        // empty. The line must still name the conversation.
        let line = turn_report_line("conv-1", "", Some("send-key-9"), &TurnOutcome::Completed);
        assert!(
            line.contains("conv-1") && line.contains("send-key-9"),
            "an id-less end still reports the conversation and the send it closes: {line:?}"
        );
    }

    #[test]
    fn effect_census_covers_every_variant_exactly_once() {
        // The sample list feeds the engine's coverage test; a variant missing
        // from it would be a coverage hole that no other test can see.
        let mut seen = vec![false; VARIANT_COUNT];
        for effect in every_variant() {
            let index = ordinal(&effect);
            assert!(
                index < VARIANT_COUNT,
                "ordinal {index} is out of range — bump census::VARIANT_COUNT \
                 after adding an Effect variant ({effect:?})"
            );
            assert!(
                !seen[index],
                "two samples share ordinal {index} ({effect:?})"
            );
            seen[index] = true;
        }
        let missing: Vec<usize> = seen
            .iter()
            .enumerate()
            .filter(|(_, covered)| !**covered)
            .map(|(index, _)| index)
            .collect();
        assert!(
            missing.is_empty(),
            "census::every_variant is missing a sample for Effect ordinal(s) {missing:?}"
        );
    }
}
