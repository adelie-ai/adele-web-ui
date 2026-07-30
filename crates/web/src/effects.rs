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

#[cfg(test)]
pub mod census {
    //! One sample of every [`Effect`] variant, and the ordinal census that keeps
    //! that sample list complete.
    //!
    //! [`ordinal`] matches exhaustively, so a new upstream variant fails to
    //! compile here; [`VARIANT_COUNT`] then makes the omission of its *sample*
    //! a test failure rather than a quiet coverage hole.

    use client_ui_common::{AdeleOutput, ContextUsageView, Effect, SelectedModel};
    use desktop_assistant_api_model as api;
    use desktop_assistant_api_model::client::{
        ConversationDetail, ConversationSummary, MessageKind,
    };

    /// How many variants [`Effect`] has. Bump it when `ordinal` gains an arm.
    pub const VARIANT_COUNT: usize = 37;

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
