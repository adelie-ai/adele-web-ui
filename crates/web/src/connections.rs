//! The Connections settings panel (issue #10): manage LLM-provider connections
//! (ollama / bedrock / openai / anthropic) — list, create, update, delete, and
//! set/rotate/clear a connection's write-only credential.
//!
//! Split like [`crate::model`] / [`crate::wire`]: the pure form ⇄
//! [`ConnectionConfigView`] mapping, credential decision, and list-render
//! helpers are transport-/view-free so they compile and unit-test on the host
//! target. The Leptos view (`#[cfg(target_arch = "wasm32")]`) and the engine
//! commands are the thin wasm shell over that logic.
//!
//! **Credential security posture.** The SPA is the first client to actually
//! send [`Command::SetConnectionSecret`]. The credential field is *write-only*:
//! it is never populated from a [`ConnectionView`] (the daemon never echoes a
//! secret — only a `has_credentials` boolean), a blank field with no explicit
//! "clear" is a no-op (we never implicitly wipe a stored secret), and an
//! explicit clear sends the empty string the daemon documents as "clear". The
//! raw value rides one command to the tailnet-only BFF and is redacted from
//! `Debug` by [`Secret`]; it is never read back.

use desktop_assistant_api_model::{
    Command, ConnectionAvailability, ConnectionConfigView, ConnectionView,
};

// ===========================================================================
// Pure logic (host-testable)
// ===========================================================================

/// The connector kinds the panel can build forms for. Mirrors the
/// [`ConnectionConfigView`] variants (and the tui/gtk `ConnectorKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectorKind {
    Anthropic,
    OpenAi,
    Bedrock,
    Ollama,
}

impl ConnectorKind {
    /// Every kind, in nav/add order.
    pub const ALL: &'static [ConnectorKind] =
        &[Self::Anthropic, Self::OpenAi, Self::Bedrock, Self::Ollama];

    /// Human-friendly label for chips / headings.
    pub fn label(self) -> &'static str {
        match self {
            Self::Anthropic => "Anthropic",
            Self::OpenAi => "OpenAI",
            Self::Bedrock => "Bedrock",
            Self::Ollama => "Ollama",
        }
    }

    /// The wire tag the daemon uses (`type =` on [`ConnectionConfigView`], and
    /// the `connector_type` string on [`ConnectionView`]).
    pub fn tag(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::Bedrock => "bedrock",
            Self::Ollama => "ollama",
        }
    }

    /// Parse a `connector_type` / `type` tag back into a kind.
    pub fn from_tag(tag: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|k| k.tag() == tag)
    }

    /// Whether this connector accepts a raw credential via
    /// [`Command::SetConnectionSecret`]. Ollama authenticates with nothing (or
    /// ambient config), so it has no credential to set; every other kind can
    /// take an API key or (Bedrock) an `ACCESS_KEY_ID:SECRET[:SESSION]` triple.
    pub fn accepts_credential(self) -> bool {
        !matches!(self, Self::Ollama)
    }

    /// Placeholder for the credential input, distinguishing Bedrock's composite
    /// form from a plain API key. Empty for kinds that take no credential.
    pub fn credential_placeholder(self) -> &'static str {
        match self {
            Self::Bedrock => "ACCESS_KEY_ID:SECRET_ACCESS_KEY[:SESSION_TOKEN]",
            Self::Anthropic | Self::OpenAi => "Paste API key (stored write-only)",
            Self::Ollama => "",
        }
    }
}

/// What to do with a connection's credential on save. Distinct from "leave it
/// alone" (`None` at the call sites), so a blank field never implicitly wipes a
/// stored secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialAction {
    /// Store this raw credential (non-empty, whitespace-trimmed).
    Set(String),
    /// Explicitly clear the stored credential (sends the empty string).
    Clear,
}

/// Config fields the form doesn't surface as inputs (timeouts, the context
/// ceiling, and Ollama's `keep_warm`). Carried through an edit round-trip so
/// saving preserves whatever the daemon had stored rather than resetting it to
/// `None` (mirrors adele-gtk's `PreservedFields`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PreservedFields {
    pub connect_timeout_secs: Option<u64>,
    pub stream_timeout_secs: Option<u64>,
    pub max_context_tokens: Option<u64>,
    pub keep_warm: Option<bool>,
}

impl PreservedFields {
    /// Extract the unsurfaced fields from an echoed config (`None` config — the
    /// create path or an older daemon — yields all-`None`).
    pub fn from_config(config: Option<&ConnectionConfigView>) -> Self {
        // Spec stub — real logic lands in the implementation commit.
        let _ = config;
        todo!("PreservedFields::from_config")
    }
}

/// The editable state of one connection form. A flat DTO: the wasm view splats
/// it into per-field reactive signals and reassembles it on submit, keeping the
/// mapping pure and testable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnForm {
    /// `Some(id)` when editing an existing connection — the id is immutable and
    /// the kind is locked. `None` when creating.
    pub editing_id: Option<String>,
    pub kind: ConnectorKind,
    pub id: String,
    pub base_url: String,
    pub api_key_env: String,
    pub aws_profile: String,
    pub region: String,
    /// Write-only credential input. Never populated from a view.
    pub secret: String,
    /// Explicit "clear the stored credential" toggle.
    pub clear_secret: bool,
    /// Unsurfaced fields carried through an edit unchanged.
    pub preserved: PreservedFields,
}

impl ConnForm {
    /// A blank create form for `kind`.
    pub fn blank(kind: ConnectorKind) -> Self {
        Self {
            editing_id: None,
            kind,
            id: String::new(),
            base_url: String::new(),
            api_key_env: String::new(),
            aws_profile: String::new(),
            region: String::new(),
            secret: String::new(),
            clear_secret: false,
            preserved: PreservedFields::default(),
        }
    }

    /// Pre-fill an edit form from a connection view: id + kind, the surfaced
    /// non-secret config fields from the echoed `config`, and the unsurfaced
    /// fields into `preserved`. The credential inputs stay blank — a stored
    /// secret is never echoed or round-tripped.
    pub fn from_view(view: &ConnectionView) -> Self {
        // Spec stub — real logic lands in the implementation commit.
        let _ = view;
        todo!("ConnForm::from_view")
    }

    /// Validate + assemble the form into the command inputs: the target id
    /// (typed for create, immutable for edit), the per-connector config, and
    /// the optional credential action. `Err` carries a human-readable reason.
    pub fn build(&self) -> Result<BuiltConnection, String> {
        // Spec stub — real logic lands in the implementation commit. Reads every
        // field so the flat DTO carries no dead weight before the impl lands.
        let _ = (
            &self.editing_id,
            self.kind,
            &self.id,
            &self.base_url,
            &self.api_key_env,
            &self.aws_profile,
            &self.region,
            &self.secret,
            self.clear_secret,
            self.preserved,
        );
        todo!("ConnForm::build")
    }
}

/// The assembled inputs for a create/update (+ optional credential) round-trip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltConnection {
    /// `Some` ⇒ `UpdateConnection`; `None` ⇒ `CreateConnection`.
    pub editing_id: Option<String>,
    /// The target connection id (immutable on edit, typed on create).
    pub id: String,
    pub config: ConnectionConfigView,
    /// The credential to set/clear after the config write, if any.
    pub credential: Option<CredentialAction>,
}

/// Decide what to do with the credential field on submit. `raw` is the field's
/// current text; `clear_requested` is a distinct explicit "clear" toggle.
///
/// - clear requested → [`CredentialAction::Clear`] (wins over any typed text);
/// - non-blank text → [`CredentialAction::Set`] (whitespace-trimmed for paste
///   hygiene — no credential format has significant surrounding whitespace);
/// - blank text, no clear → `None`: we never implicitly wipe a stored secret.
pub fn credential_action(raw: &str, clear_requested: bool) -> Option<CredentialAction> {
    // Spec stub — real logic lands in the implementation commit.
    let _ = (raw, clear_requested);
    todo!("credential_action")
}

/// Build the [`Command::SetConnectionSecret`] for a credential action. A
/// [`CredentialAction::Clear`] sends the empty string (the daemon's documented
/// "clear" signal); the value is wrapped in [`Secret`] so it can't leak via
/// `Debug`.
pub fn secret_command(id: String, action: CredentialAction) -> Command {
    // Spec stub — real logic lands in the implementation commit.
    let _ = (&id, &action);
    todo!("secret_command")
}

/// A one-line availability summary for a connection card.
pub fn availability_label(availability: &ConnectionAvailability) -> String {
    // Spec stub — real logic lands in the implementation commit.
    let _ = availability;
    todo!("availability_label")
}

/// Whether a connection is currently healthy (drives the status dot colour).
pub fn availability_is_ok(availability: &ConnectionAvailability) -> bool {
    matches!(availability, ConnectionAvailability::Ok)
}

/// The credential-state label for a card / form (never the secret itself).
pub fn credentials_label(has_credentials: bool) -> &'static str {
    // Spec stub — real logic lands in the implementation commit.
    let _ = has_credentials;
    todo!("credentials_label")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn openai_config() -> ConnectionConfigView {
        ConnectionConfigView::OpenAi {
            base_url: Some("https://api.openai.com/v1".into()),
            api_key_env: Some("OPENAI_WORK_KEY".into()),
            connect_timeout_secs: None,
            stream_timeout_secs: None,
            max_context_tokens: None,
        }
    }

    fn view(id: &str, ty: &str, config: Option<ConnectionConfigView>) -> ConnectionView {
        ConnectionView {
            id: id.into(),
            connector_type: ty.into(),
            display_label: format!("{id} ({ty})"),
            availability: ConnectionAvailability::Ok,
            has_credentials: true,
            config,
        }
    }

    // --- ConnectorKind --------------------------------------------------------

    #[test]
    fn connector_kind_round_trips_via_tag() {
        for kind in ConnectorKind::ALL {
            assert_eq!(ConnectorKind::from_tag(kind.tag()), Some(*kind));
        }
        assert_eq!(ConnectorKind::from_tag("carrier-pigeon"), None);
    }

    #[test]
    fn connector_kind_all_covers_four_variants() {
        assert_eq!(ConnectorKind::ALL.len(), 4);
    }

    #[test]
    fn connector_kind_labels() {
        assert_eq!(ConnectorKind::Anthropic.label(), "Anthropic");
        assert_eq!(ConnectorKind::OpenAi.label(), "OpenAI");
        assert_eq!(ConnectorKind::Bedrock.label(), "Bedrock");
        assert_eq!(ConnectorKind::Ollama.label(), "Ollama");
    }

    #[test]
    fn accepts_credential_excludes_only_ollama() {
        assert!(ConnectorKind::Anthropic.accepts_credential());
        assert!(ConnectorKind::OpenAi.accepts_credential());
        assert!(ConnectorKind::Bedrock.accepts_credential());
        assert!(!ConnectorKind::Ollama.accepts_credential());
    }

    #[test]
    fn credential_placeholder_bedrock_mentions_access_key() {
        assert!(
            ConnectorKind::Bedrock
                .credential_placeholder()
                .contains("ACCESS_KEY_ID")
        );
        assert!(ConnectorKind::Ollama.credential_placeholder().is_empty());
    }

    // --- credential_action ----------------------------------------------------

    #[test]
    fn credential_action_sets_when_nonempty() {
        assert_eq!(
            credential_action("sk-abc123", false),
            Some(CredentialAction::Set("sk-abc123".into()))
        );
    }

    #[test]
    fn credential_action_trims_surrounding_whitespace() {
        assert_eq!(
            credential_action("  sk-abc123\n", false),
            Some(CredentialAction::Set("sk-abc123".into()))
        );
    }

    #[test]
    fn credential_action_clears_when_requested() {
        assert_eq!(credential_action("", true), Some(CredentialAction::Clear));
    }

    #[test]
    fn credential_action_clear_wins_over_typed_text() {
        // An explicit clear removes the secret regardless of stray field text.
        assert_eq!(
            credential_action("ignored", true),
            Some(CredentialAction::Clear)
        );
    }

    #[test]
    fn credential_action_is_none_when_blank_and_no_clear() {
        // The write-only no-op: a blank field must never wipe a stored secret.
        assert_eq!(credential_action("", false), None);
        assert_eq!(credential_action("   \t ", false), None);
    }

    // --- secret_command (wire shape + redaction) ------------------------------

    #[test]
    fn secret_command_set_wire_shape() {
        let cmd = secret_command("work".into(), CredentialAction::Set("sk-xyz".into()));
        let json = serde_json::to_string(&cmd).expect("serializes");
        assert_eq!(
            json,
            r#"{"set_connection_secret":{"id":"work","credential":"sk-xyz"}}"#
        );
    }

    #[test]
    fn secret_command_clear_sends_empty_string() {
        let cmd = secret_command("work".into(), CredentialAction::Clear);
        let json = serde_json::to_string(&cmd).expect("serializes");
        assert_eq!(
            json,
            r#"{"set_connection_secret":{"id":"work","credential":""}}"#
        );
    }

    #[test]
    fn secret_command_credential_redacted_in_debug() {
        let cmd = secret_command("work".into(), CredentialAction::Set("sk-topsecret".into()));
        let dump = format!("{cmd:?}");
        assert!(
            !dump.contains("sk-topsecret"),
            "credential leaked into Debug: {dump}"
        );
    }

    // --- ConnForm::build ------------------------------------------------------

    #[test]
    fn build_rejects_blank_id_on_create() {
        let form = ConnForm::blank(ConnectorKind::OpenAi);
        assert!(form.build().is_err());
    }

    #[test]
    fn build_rejects_invalid_slug() {
        let mut form = ConnForm::blank(ConnectorKind::OpenAi);
        form.id = "has spaces".into();
        assert!(form.build().is_err());
        form.id = "bad/slash".into();
        assert!(form.build().is_err());
    }

    #[test]
    fn build_openai_config_and_id() {
        let mut form = ConnForm::blank(ConnectorKind::OpenAi);
        form.id = "work".into();
        form.base_url = "https://api.openai.com/v1".into();
        form.api_key_env = "OPENAI_API_KEY".into();
        let built = form.build().expect("valid form builds");
        assert_eq!(built.editing_id, None);
        assert_eq!(built.id, "work");
        match built.config {
            ConnectionConfigView::OpenAi {
                base_url,
                api_key_env,
                ..
            } => {
                assert_eq!(base_url.as_deref(), Some("https://api.openai.com/v1"));
                assert_eq!(api_key_env.as_deref(), Some("OPENAI_API_KEY"));
            }
            other => panic!("expected OpenAi, got {other:?}"),
        }
    }

    #[test]
    fn build_anthropic_config() {
        let mut form = ConnForm::blank(ConnectorKind::Anthropic);
        form.id = "claude".into();
        form.api_key_env = "ANTHROPIC_API_KEY".into();
        let built = form.build().expect("valid form builds");
        assert!(matches!(
            built.config,
            ConnectionConfigView::Anthropic { .. }
        ));
    }

    #[test]
    fn build_bedrock_config() {
        let mut form = ConnForm::blank(ConnectorKind::Bedrock);
        form.id = "aws".into();
        form.aws_profile = "prod".into();
        form.region = "us-west-2".into();
        let built = form.build().expect("valid form builds");
        match built.config {
            ConnectionConfigView::Bedrock {
                aws_profile,
                region,
                ..
            } => {
                assert_eq!(aws_profile.as_deref(), Some("prod"));
                assert_eq!(region.as_deref(), Some("us-west-2"));
            }
            other => panic!("expected Bedrock, got {other:?}"),
        }
    }

    #[test]
    fn build_ollama_config_never_carries_credential() {
        let mut form = ConnForm::blank(ConnectorKind::Ollama);
        form.id = "local".into();
        form.base_url = "http://127.0.0.1:11434".into();
        // Even if some stray secret text is present, Ollama takes no credential.
        form.secret = "should-be-ignored".into();
        let built = form.build().expect("valid form builds");
        assert!(matches!(built.config, ConnectionConfigView::Ollama { .. }));
        assert_eq!(built.credential, None);
    }

    #[test]
    fn build_uses_editing_id_and_ignores_typed_id() {
        let mut form = ConnForm::blank(ConnectorKind::OpenAi);
        form.editing_id = Some("locked".into());
        form.id = "typed-but-ignored".into();
        let built = form.build().expect("valid form builds");
        assert_eq!(built.editing_id.as_deref(), Some("locked"));
        assert_eq!(built.id, "locked");
    }

    #[test]
    fn build_bundles_credential_for_new_api_connection() {
        let mut form = ConnForm::blank(ConnectorKind::OpenAi);
        form.id = "work".into();
        form.secret = "sk-live".into();
        let built = form.build().expect("valid form builds");
        assert_eq!(
            built.credential,
            Some(CredentialAction::Set("sk-live".into()))
        );
    }

    #[test]
    fn build_bundles_clear_credential() {
        let mut form = ConnForm::blank(ConnectorKind::Anthropic);
        form.editing_id = Some("claude".into());
        form.clear_secret = true;
        let built = form.build().expect("valid form builds");
        assert_eq!(built.credential, Some(CredentialAction::Clear));
    }

    // --- ConnForm::from_view --------------------------------------------------

    #[test]
    fn from_view_prefills_openai_and_sets_editing() {
        let form = ConnForm::from_view(&view("work", "openai", Some(openai_config())));
        assert_eq!(form.editing_id.as_deref(), Some("work"));
        assert_eq!(form.id, "work");
        assert_eq!(form.kind, ConnectorKind::OpenAi);
        assert_eq!(form.base_url, "https://api.openai.com/v1");
        assert_eq!(form.api_key_env, "OPENAI_WORK_KEY");
    }

    #[test]
    fn from_view_prefills_bedrock() {
        let config = ConnectionConfigView::Bedrock {
            aws_profile: Some("prod".into()),
            region: Some("eu-central-1".into()),
            base_url: None,
            connect_timeout_secs: None,
            stream_timeout_secs: None,
            max_context_tokens: None,
        };
        let form = ConnForm::from_view(&view("aws", "bedrock", Some(config)));
        assert_eq!(form.kind, ConnectorKind::Bedrock);
        assert_eq!(form.aws_profile, "prod");
        assert_eq!(form.region, "eu-central-1");
        assert_eq!(form.base_url, "");
    }

    #[test]
    fn from_view_blank_fields_when_config_none() {
        // Older daemon that omits `config`: kind + id set, fields blank.
        let form = ConnForm::from_view(&view("work", "anthropic", None));
        assert_eq!(form.kind, ConnectorKind::Anthropic);
        assert_eq!(form.id, "work");
        assert_eq!(form.base_url, "");
        assert_eq!(form.api_key_env, "");
    }

    #[test]
    fn from_view_never_prefills_secret() {
        // has_credentials is true, but the secret input stays empty and no clear
        // is staged — a stored secret is never echoed or round-tripped.
        let form = ConnForm::from_view(&view("work", "openai", Some(openai_config())));
        assert_eq!(form.secret, "");
        assert!(!form.clear_secret);
    }

    #[test]
    fn from_view_preserves_unsurfaced_fields_through_build() {
        // The form has no inputs for timeouts / context ceiling / keep_warm, so
        // an edit save must carry the daemon's stored values through unchanged.
        let config = ConnectionConfigView::Ollama {
            base_url: Some("http://localhost:11434".into()),
            connect_timeout_secs: Some(5),
            stream_timeout_secs: Some(120),
            keep_warm: Some(true),
            max_context_tokens: Some(8192),
        };
        let form = ConnForm::from_view(&view("local", "ollama", Some(config)));
        let built = form.build().expect("valid form builds");
        match built.config {
            ConnectionConfigView::Ollama {
                base_url,
                connect_timeout_secs,
                stream_timeout_secs,
                keep_warm,
                max_context_tokens,
            } => {
                assert_eq!(base_url.as_deref(), Some("http://localhost:11434"));
                assert_eq!(connect_timeout_secs, Some(5));
                assert_eq!(stream_timeout_secs, Some(120));
                assert_eq!(keep_warm, Some(true));
                assert_eq!(max_context_tokens, Some(8192));
            }
            other => panic!("expected Ollama, got {other:?}"),
        }
    }

    // --- PreservedFields ------------------------------------------------------

    #[test]
    fn preserved_fields_from_config_variants() {
        let bedrock = ConnectionConfigView::Bedrock {
            aws_profile: None,
            region: Some("us-west-2".into()),
            base_url: None,
            connect_timeout_secs: Some(3),
            stream_timeout_secs: None,
            max_context_tokens: Some(200_000),
        };
        let p = PreservedFields::from_config(Some(&bedrock));
        assert_eq!(p.connect_timeout_secs, Some(3));
        assert_eq!(p.max_context_tokens, Some(200_000));
        assert_eq!(p.keep_warm, None);

        let ollama = ConnectionConfigView::Ollama {
            base_url: None,
            connect_timeout_secs: None,
            stream_timeout_secs: None,
            keep_warm: Some(false),
            max_context_tokens: None,
        };
        assert_eq!(
            PreservedFields::from_config(Some(&ollama)).keep_warm,
            Some(false)
        );

        // Create path: nothing stored.
        assert_eq!(
            PreservedFields::from_config(None),
            PreservedFields::default()
        );
    }

    // --- List-render helpers --------------------------------------------------

    #[test]
    fn availability_label_reflects_state() {
        assert_eq!(availability_label(&ConnectionAvailability::Ok), "Available");
        assert_eq!(
            availability_label(&ConnectionAvailability::Unavailable {
                reason: "no api key".into()
            }),
            "Unavailable: no api key"
        );
    }

    #[test]
    fn availability_is_ok_only_for_ok() {
        assert!(availability_is_ok(&ConnectionAvailability::Ok));
        assert!(!availability_is_ok(&ConnectionAvailability::Unavailable {
            reason: "x".into()
        }));
    }

    #[test]
    fn credentials_label_reflects_flag() {
        assert_ne!(credentials_label(true), credentials_label(false));
        assert!(!credentials_label(false).is_empty());
    }
}
