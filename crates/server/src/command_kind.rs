//! The command's variant name, safe to record on the `daemon.call` span at INFO under
//! the D10 level contract - never a field value.
//!
//! Split into its own module because the match below is long (one arm per `Command`
//! variant) and does not belong inside `forward.rs`'s forwarding logic.

use desktop_assistant_api_model as api;

/// The command's variant name only.
///
/// A literal `match`, not a formatted-and-split `Debug` string: the previous
/// implementation built the whole command's `Debug` representation - prompt, tool
/// arguments and all - on every non-streaming call just to read a name a `match` gives
/// for free, with no allocation at all for a known variant (review finding on
/// `adele-web-ui#91`).
///
/// Exhaustive, deliberately, with no wildcard arm: `Command` is defined in
/// `desktop-assistant`, a dependency this crate does not control, and a variant added
/// there must fail this build rather than silently mislabel a metric as some vague
/// fallback until someone notices. Add the missing arm when the compiler names it.
pub fn command_kind(cmd: &api::Command) -> &'static str {
    match cmd {
        api::Command::Ping => "Ping",
        api::Command::GetStatus => "GetStatus",
        api::Command::GetConfig => "GetConfig",
        api::Command::SetConfig { .. } => "SetConfig",
        api::Command::CreateConversation { .. } => "CreateConversation",
        api::Command::ListConversations { .. } => "ListConversations",
        api::Command::GetConversation { .. } => "GetConversation",
        api::Command::GetMessages { .. } => "GetMessages",
        api::Command::DeleteConversation { .. } => "DeleteConversation",
        api::Command::RenameConversation { .. } => "RenameConversation",
        api::Command::ArchiveConversation { .. } => "ArchiveConversation",
        api::Command::UnarchiveConversation { .. } => "UnarchiveConversation",
        api::Command::ClearAllHistory => "ClearAllHistory",
        api::Command::SendMessage { .. } => "SendMessage",
        api::Command::SetConversationPersonality { .. } => "SetConversationPersonality",
        api::Command::SetConversationToolGate { .. } => "SetConversationToolGate",
        api::Command::SetApiKey { .. } => "SetApiKey",
        api::Command::GetEmbeddingsSettings => "GetEmbeddingsSettings",
        api::Command::SetEmbeddingsSettings { .. } => "SetEmbeddingsSettings",
        api::Command::GetConnectorDefaults { .. } => "GetConnectorDefaults",
        api::Command::GetDatabaseSettings => "GetDatabaseSettings",
        api::Command::SetDatabaseSettings { .. } => "SetDatabaseSettings",
        api::Command::GetBackendTasksSettings => "GetBackendTasksSettings",
        api::Command::SetBackendTasksSettings { .. } => "SetBackendTasksSettings",
        api::Command::GetWsAuthSettings => "GetWsAuthSettings",
        api::Command::SetWsAuthSettings { .. } => "SetWsAuthSettings",
        api::Command::ListConnections => "ListConnections",
        api::Command::CreateConnection { .. } => "CreateConnection",
        api::Command::UpdateConnection { .. } => "UpdateConnection",
        api::Command::DeleteConnection { .. } => "DeleteConnection",
        api::Command::SetConnectionSecret { .. } => "SetConnectionSecret",
        api::Command::ListAvailableModels { .. } => "ListAvailableModels",
        api::Command::GetPurposes => "GetPurposes",
        api::Command::SetPurpose { .. } => "SetPurpose",
        api::Command::GetToolUsage { .. } => "GetToolUsage",
        api::Command::ListKnowledgeEntries { .. } => "ListKnowledgeEntries",
        api::Command::GetKnowledgeEntry { .. } => "GetKnowledgeEntry",
        api::Command::SearchKnowledgeEntries { .. } => "SearchKnowledgeEntries",
        api::Command::CreateKnowledgeEntry { .. } => "CreateKnowledgeEntry",
        api::Command::UpdateKnowledgeEntry { .. } => "UpdateKnowledgeEntry",
        api::Command::DeleteKnowledgeEntry { .. } => "DeleteKnowledgeEntry",
        api::Command::GetKnowledgeTrashCount => "GetKnowledgeTrashCount",
        api::Command::EmptyKnowledgeTrash => "EmptyKnowledgeTrash",
        api::Command::ListSkills { .. } => "ListSkills",
        api::Command::SetSkillApproval { .. } => "SetSkillApproval",
        api::Command::StartKnowledgeMaintenance { .. } => "StartKnowledgeMaintenance",
        api::Command::ListMcpServers => "ListMcpServers",
        api::Command::AddMcpServer { .. } => "AddMcpServer",
        api::Command::RemoveMcpServer { .. } => "RemoveMcpServer",
        api::Command::SetMcpServerEnabled { .. } => "SetMcpServerEnabled",
        api::Command::McpServerAction { .. } => "McpServerAction",
        api::Command::UpsertMcpServer { .. } => "UpsertMcpServer",
        api::Command::SetMcpSecret { .. } => "SetMcpSecret",
        api::Command::ListServiceAccounts => "ListServiceAccounts",
        api::Command::UpsertServiceAccount { .. } => "UpsertServiceAccount",
        api::Command::RemoveServiceAccount { .. } => "RemoveServiceAccount",
        api::Command::ListBackgroundTasks { .. } => "ListBackgroundTasks",
        api::Command::GetBackgroundTask { .. } => "GetBackgroundTask",
        api::Command::CancelBackgroundTask { .. } => "CancelBackgroundTask",
        api::Command::GetBackgroundTaskLogs { .. } => "GetBackgroundTaskLogs",
        api::Command::SubscribeBackgroundTasks => "SubscribeBackgroundTasks",
        api::Command::UnsubscribeBackgroundTasks => "UnsubscribeBackgroundTasks",
        api::Command::SubscribeConversations { .. } => "SubscribeConversations",
        api::Command::SpawnStandaloneAgent { .. } => "SpawnStandaloneAgent",
        api::Command::GetConversationScratchpad { .. } => "GetConversationScratchpad",
        api::Command::SetScratchpadNote { .. } => "SetScratchpadNote",
        api::Command::DeleteScratchpadNotes { .. } => "DeleteScratchpadNotes",
        api::Command::RegisterClientTools { .. } => "RegisterClientTools",
        api::Command::ClientToolResult { .. } => "ClientToolResult",
        api::Command::ListNegativeMemories => "ListNegativeMemories",
        api::Command::GetNegativeMemory { .. } => "GetNegativeMemory",
        api::Command::ClearNegativeMemory { .. } => "ClearNegativeMemory",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Acceptance: `command_kind` carries only the variant name, never a field value -
    /// the mechanism the `daemon.call` span's `command` field relies on to stay
    /// content-free for every command, not only `SendMessage`.
    #[test]
    fn command_kind_strips_field_values() {
        let cmd = api::Command::RenameConversation {
            id: "c1".to_string(),
            title: "MARKER_USER_SUPPLIED_TITLE".to_string(),
        };
        assert_eq!(command_kind(&cmd), "RenameConversation");
    }

    #[test]
    fn command_kind_of_a_unit_variant_is_the_variant_name() {
        assert_eq!(command_kind(&api::Command::Ping), "Ping");
    }

    #[test]
    fn command_kind_of_send_message_is_send_message() {
        // SendMessage never actually reaches this function in production (the
        // dispatcher special-cases it before `handle_command`, and forward.rs's
        // streaming path uses a literal "SendMessage" instead) - matched anyway so the
        // arm list stays exhaustive against every variant `Command` actually has.
        let cmd = api::Command::SendMessage {
            conversation_id: "c1".to_string(),
            content: "MARKER_PROMPT_TEXT".to_string(),
            override_selection: None,
            system_refinement: String::new(),
            client_context: None,
            idempotency_key: None,
            turn_id: None,
            traceparent: None,
        };
        assert_eq!(command_kind(&cmd), "SendMessage");
    }
}
