//! Pure model-selection helpers, kept transport-/view-free so they compile and
//! unit-test on the host target (like [`crate::wire`]).
//!
//! The per-conversation model picker (issue #9) stages a client-side
//! *override* that rides on the next `SendMessage`; the daemon then pins it as
//! the conversation's stored selection and later turns inherit it — there is no
//! separate "set model" command. These helpers encode the small amount of pure
//! logic that decision needs (precedence, override construction, listing
//! filtering), matching the GTK client's `resolve_active` / `current_override`
//! so the two stay behaviourally identical.

use client_ui_common::SelectedModel;
use desktop_assistant_api_model::{
    ConversationModelSelectionView, EffortLevel, ModelListing, SendPromptOverride,
};

/// Resolve the picker's *active* selection: a conversation's stored selection
/// wins; otherwise fall back to the resolved interactive-purpose default.
///
/// Mirrors adele-gtk's `resolve_active` (`stored.or(default)`). Because the
/// active selection is what [`override_for_send`] turns into the next send's
/// override, a conversation sitting on the default pins that default on its
/// first message — the documented, intended behaviour.
pub fn resolve_active(
    stored: Option<SelectedModel>,
    default: Option<SelectedModel>,
) -> Option<SelectedModel> {
    stored.or(default)
}

/// Build the override to attach to the next `SendMessage` from the active
/// selection plus the (optional) effort. `None` when nothing is actively
/// selected — the daemon then falls back to the conversation's stored selection
/// or the interactive purpose.
pub fn override_for_send(
    active: Option<&SelectedModel>,
    effort: Option<EffortLevel>,
) -> Option<SendPromptOverride> {
    active.map(|sel| SendPromptOverride {
        connection_id: sel.connection_id.clone(),
        model_id: sel.model_id.clone(),
        effort,
    })
}

/// Drop embedding-only models: they can't answer a chat turn, so they must not
/// appear in the picker (matches the GTK/`select_models_dialog` filter).
pub fn chat_capable(listings: Vec<ModelListing>) -> Vec<ModelListing> {
    listings
        .into_iter()
        .filter(|l| !l.model.capabilities.embedding)
        .collect()
}

/// Project a conversation's stored selection onto the picker's `(connection,
/// model)` identity, discarding effort (which the effort selector tracks
/// separately).
pub fn stored_to_selected(sel: &ConversationModelSelectionView) -> SelectedModel {
    SelectedModel {
        connection_id: sel.connection_id.clone(),
        model_id: sel.model_id.clone(),
    }
}

/// Human-friendly context window for a capability badge: `128000 -> "128K"`,
/// `1000000 -> "1M"`. Odd (non-round) values render verbatim so nothing is ever
/// misreported.
pub fn format_context_limit(tokens: u64) -> String {
    if tokens >= 1_000_000 && tokens.is_multiple_of(1_000_000) {
        format!("{}M", tokens / 1_000_000)
    } else if tokens >= 1_000 && tokens.is_multiple_of(1_000) {
        format!("{}K", tokens / 1_000)
    } else {
        tokens.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desktop_assistant_api_model::{ModelCapabilitiesView, ModelInfoView};

    fn selected(conn: &str, model: &str) -> SelectedModel {
        SelectedModel {
            connection_id: conn.into(),
            model_id: model.into(),
        }
    }

    fn listing(conn: &str, model: &str, embedding: bool) -> ModelListing {
        ModelListing {
            connection_id: conn.into(),
            connection_label: format!("{conn} (test)"),
            model: ModelInfoView {
                id: model.into(),
                display_name: model.into(),
                context_limit: None,
                capabilities: ModelCapabilitiesView {
                    embedding,
                    ..Default::default()
                },
            },
        }
    }

    #[test]
    fn resolve_active_prefers_stored_over_default() {
        let stored = selected("work", "claude");
        let default = selected("openai", "gpt-4o");
        assert_eq!(
            resolve_active(Some(stored.clone()), Some(default)),
            Some(stored)
        );
    }

    #[test]
    fn resolve_active_falls_back_to_default() {
        let default = selected("openai", "gpt-4o");
        assert_eq!(resolve_active(None, Some(default.clone())), Some(default));
    }

    #[test]
    fn resolve_active_is_none_without_either() {
        assert_eq!(resolve_active(None, None), None);
    }

    #[test]
    fn override_for_send_carries_active_and_effort() {
        let active = selected("work", "claude");
        let ov = override_for_send(Some(&active), Some(EffortLevel::High))
            .expect("an active selection yields an override");
        assert_eq!(ov.connection_id, "work");
        assert_eq!(ov.model_id, "claude");
        assert_eq!(ov.effort, Some(EffortLevel::High));
    }

    #[test]
    fn override_for_send_is_none_without_active() {
        assert!(override_for_send(None, Some(EffortLevel::Low)).is_none());
    }

    #[test]
    fn chat_capable_drops_embedding_only_models() {
        let listings = vec![
            listing("openai", "gpt-4o", false),
            listing("openai", "text-embedding-3", true),
        ];
        let filtered = chat_capable(listings);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].model.id, "gpt-4o");
    }

    #[test]
    fn format_context_limit_uses_round_units() {
        assert_eq!(format_context_limit(128_000), "128K");
        assert_eq!(format_context_limit(200_000), "200K");
        assert_eq!(format_context_limit(1_000_000), "1M");
        assert_eq!(format_context_limit(2_000_000), "2M");
        // Non-round values render verbatim rather than being misreported.
        assert_eq!(format_context_limit(131_072), "131072");
        assert_eq!(format_context_limit(512), "512");
    }

    #[test]
    fn stored_to_selected_drops_effort() {
        let stored = ConversationModelSelectionView {
            connection_id: "work".into(),
            model_id: "claude".into(),
            effort: Some(EffortLevel::Medium),
        };
        assert_eq!(stored_to_selected(&stored), selected("work", "claude"));
    }
}
