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
    Command, ConnectionAvailability, ConnectionConfigView, ConnectionView, Secret,
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
        match config {
            Some(
                ConnectionConfigView::Anthropic {
                    connect_timeout_secs,
                    stream_timeout_secs,
                    max_context_tokens,
                    ..
                }
                | ConnectionConfigView::OpenAi {
                    connect_timeout_secs,
                    stream_timeout_secs,
                    max_context_tokens,
                    ..
                }
                | ConnectionConfigView::Bedrock {
                    connect_timeout_secs,
                    stream_timeout_secs,
                    max_context_tokens,
                    ..
                },
            ) => Self {
                connect_timeout_secs: *connect_timeout_secs,
                stream_timeout_secs: *stream_timeout_secs,
                max_context_tokens: *max_context_tokens,
                keep_warm: None,
            },
            Some(ConnectionConfigView::Ollama {
                connect_timeout_secs,
                stream_timeout_secs,
                max_context_tokens,
                keep_warm,
                ..
            }) => Self {
                connect_timeout_secs: *connect_timeout_secs,
                stream_timeout_secs: *stream_timeout_secs,
                max_context_tokens: *max_context_tokens,
                keep_warm: *keep_warm,
            },
            None => Self::default(),
        }
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
    /// Write-only credential input for the single-field (api-key) connectors.
    /// Never populated from a view.
    pub secret: String,
    /// Bedrock's separate write-only credential inputs, joined on save into the
    /// `ACCESS_KEY_ID:SECRET_ACCESS_KEY[:SESSION_TOKEN]` string. Never populated
    /// from a view (the daemon never echoes a stored secret).
    pub aws_access_key_id: String,
    pub aws_secret_access_key: String,
    pub aws_session_token: String,
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
            aws_access_key_id: String::new(),
            aws_secret_access_key: String::new(),
            aws_session_token: String::new(),
            clear_secret: false,
            preserved: PreservedFields::default(),
        }
    }

    /// Pre-fill an edit form from a connection view: id + kind, the surfaced
    /// non-secret config fields from the echoed `config`, and the unsurfaced
    /// fields into `preserved`. The credential inputs stay blank — a stored
    /// secret is never echoed or round-tripped.
    pub fn from_view(view: &ConnectionView) -> Self {
        // Fall back to Anthropic on an unrecognized connector_type so the form
        // is still usable; a mismatched/absent config just leaves fields blank.
        let kind =
            ConnectorKind::from_tag(&view.connector_type).unwrap_or(ConnectorKind::Anthropic);
        let config = view.config.as_ref();
        // Only pre-fill from a config whose variant matches `kind`, so a
        // mismatch never leaks a value across connector types.
        let matched = config.filter(|c| c.connector_type() == kind.tag());

        let (base_url, api_key_env, aws_profile, region) = match matched {
            Some(ConnectionConfigView::Anthropic {
                base_url,
                api_key_env,
                ..
            })
            | Some(ConnectionConfigView::OpenAi {
                base_url,
                api_key_env,
                ..
            }) => (
                base_url.clone().unwrap_or_default(),
                api_key_env.clone().unwrap_or_default(),
                String::new(),
                String::new(),
            ),
            Some(ConnectionConfigView::Bedrock {
                aws_profile,
                region,
                base_url,
                ..
            }) => (
                base_url.clone().unwrap_or_default(),
                String::new(),
                aws_profile.clone().unwrap_or_default(),
                region.clone().unwrap_or_default(),
            ),
            Some(ConnectionConfigView::Ollama { base_url, .. }) => (
                base_url.clone().unwrap_or_default(),
                String::new(),
                String::new(),
                String::new(),
            ),
            None => (String::new(), String::new(), String::new(), String::new()),
        };

        Self {
            editing_id: Some(view.id.clone()),
            kind,
            id: view.id.clone(),
            base_url,
            api_key_env,
            aws_profile,
            region,
            secret: String::new(),
            aws_access_key_id: String::new(),
            aws_secret_access_key: String::new(),
            aws_session_token: String::new(),
            clear_secret: false,
            preserved: PreservedFields::from_config(matched),
        }
    }

    /// Validate + assemble the form into the command inputs: the target id
    /// (typed for create, immutable for edit), the per-connector config, and
    /// the optional credential action. `Err` carries a human-readable reason.
    pub fn build(&self) -> Result<BuiltConnection, String> {
        let id = match &self.editing_id {
            // Edit: the id is immutable and already validated by the daemon.
            Some(existing) => existing.clone(),
            // Create: validate the freshly-typed slug.
            None => {
                let typed = self.id.trim().to_string();
                validate_slug(&typed)?;
                typed
            }
        };
        let credential = match self.kind {
            // Bedrock takes three separate inputs, joined into the daemon's
            // `ACCESS_KEY_ID:SECRET_ACCESS_KEY[:SESSION_TOKEN]` credential string.
            ConnectorKind::Bedrock => bedrock_credential_action(
                &self.aws_access_key_id,
                &self.aws_secret_access_key,
                &self.aws_session_token,
                self.clear_secret,
            ),
            // The single-field api-key connectors send their raw key as-is.
            ConnectorKind::Anthropic | ConnectorKind::OpenAi => {
                credential_action(&self.secret, self.clear_secret)
            }
            // Ollama takes no credential.
            ConnectorKind::Ollama => None,
        };
        Ok(BuiltConnection {
            editing_id: self.editing_id.clone(),
            id,
            config: self.config(),
            credential,
        })
    }

    /// Assemble the per-connector [`ConnectionConfigView`] from the surfaced
    /// inputs plus the round-tripped [`PreservedFields`].
    fn config(&self) -> ConnectionConfigView {
        let opt = |s: &str| {
            let t = s.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        };
        let p = self.preserved;
        match self.kind {
            ConnectorKind::Anthropic => ConnectionConfigView::Anthropic {
                base_url: opt(&self.base_url),
                api_key_env: opt(&self.api_key_env),
                connect_timeout_secs: p.connect_timeout_secs,
                stream_timeout_secs: p.stream_timeout_secs,
                max_context_tokens: p.max_context_tokens,
            },
            ConnectorKind::OpenAi => ConnectionConfigView::OpenAi {
                base_url: opt(&self.base_url),
                api_key_env: opt(&self.api_key_env),
                connect_timeout_secs: p.connect_timeout_secs,
                stream_timeout_secs: p.stream_timeout_secs,
                max_context_tokens: p.max_context_tokens,
            },
            ConnectorKind::Bedrock => ConnectionConfigView::Bedrock {
                aws_profile: opt(&self.aws_profile),
                region: opt(&self.region),
                base_url: opt(&self.base_url),
                connect_timeout_secs: p.connect_timeout_secs,
                stream_timeout_secs: p.stream_timeout_secs,
                max_context_tokens: p.max_context_tokens,
            },
            ConnectorKind::Ollama => ConnectionConfigView::Ollama {
                base_url: opt(&self.base_url),
                connect_timeout_secs: p.connect_timeout_secs,
                stream_timeout_secs: p.stream_timeout_secs,
                keep_warm: p.keep_warm,
                max_context_tokens: p.max_context_tokens,
            },
        }
    }
}

/// Validate a connection id slug: non-empty, and only letters, digits, `-`, `_`
/// (mirrors the gtk dialog + the daemon's slug contract).
fn validate_slug(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("Connection id is required.".to_string());
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("Id may only contain letters, digits, '-', and '_'.".to_string());
    }
    Ok(())
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
    if clear_requested {
        return Some(CredentialAction::Clear);
    }
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(CredentialAction::Set(trimmed.to_string()))
    }
}

/// Join Bedrock's three credential inputs into the daemon's
/// `ACCESS_KEY_ID:SECRET_ACCESS_KEY[:SESSION_TOKEN]` string.
///
/// Each field is whitespace-trimmed (paste hygiene — none of the parts carries
/// significant surrounding whitespace). Returns `None` unless *both* required
/// parts (access key id + secret access key) are present, so a partial or
/// all-blank entry is "no credential" rather than a malformed half-string. The
/// optional session token is appended only when non-blank, so a two-part
/// credential never carries a dangling trailing colon.
pub fn join_bedrock_credential(
    access_key_id: &str,
    secret_access_key: &str,
    session_token: &str,
) -> Option<String> {
    let access = access_key_id.trim();
    let secret = secret_access_key.trim();
    let session = session_token.trim();
    if access.is_empty() || secret.is_empty() {
        return None;
    }
    Some(if session.is_empty() {
        format!("{access}:{secret}")
    } else {
        format!("{access}:{secret}:{session}")
    })
}

/// Decide the credential action for Bedrock's separate-fields form. An explicit
/// clear wins over any typed text (mirrors [`credential_action`]); otherwise the
/// three fields are joined via [`join_bedrock_credential`] — a complete pair
/// becomes [`CredentialAction::Set`], anything short of it leaves the stored
/// credential untouched (`None`).
pub fn bedrock_credential_action(
    access_key_id: &str,
    secret_access_key: &str,
    session_token: &str,
    clear_requested: bool,
) -> Option<CredentialAction> {
    if clear_requested {
        return Some(CredentialAction::Clear);
    }
    join_bedrock_credential(access_key_id, secret_access_key, session_token)
        .map(CredentialAction::Set)
}

/// Build the [`Command::SetConnectionSecret`] for a credential action. A
/// [`CredentialAction::Clear`] sends the empty string (the daemon's documented
/// "clear" signal); the value is wrapped in [`Secret`] so it can't leak via
/// `Debug`.
pub fn secret_command(id: String, action: CredentialAction) -> Command {
    let credential = match action {
        CredentialAction::Set(value) => value,
        CredentialAction::Clear => String::new(),
    };
    Command::SetConnectionSecret {
        id,
        credential: Secret(credential),
    }
}

/// A one-line availability summary for a connection card.
pub fn availability_label(availability: &ConnectionAvailability) -> String {
    match availability {
        ConnectionAvailability::Ok => "Available".to_string(),
        ConnectionAvailability::Unavailable { reason } => format!("Unavailable: {reason}"),
    }
}

/// Whether a connection is currently healthy (drives the status dot colour).
pub fn availability_is_ok(availability: &ConnectionAvailability) -> bool {
    matches!(availability, ConnectionAvailability::Ok)
}

/// The credential-state label for a card / form (never the secret itself).
pub fn credentials_label(has_credentials: bool) -> &'static str {
    if has_credentials {
        "Credential stored"
    } else {
        "No credential"
    }
}

// ===========================================================================
// Leptos view (wasm only)
// ===========================================================================

#[cfg(target_arch = "wasm32")]
pub use view::connections_panel;

#[cfg(target_arch = "wasm32")]
mod view {
    use std::rc::Rc;

    use leptos::prelude::*;

    use super::{
        ConnForm, ConnectorKind, PreservedFields, availability_is_ok, availability_label,
        credentials_label,
    };
    use crate::engine::{ActionDone, ModelsRefreshed, ViewSignals};
    use crate::settings::EngineHandle;
    use desktop_assistant_api_model::ConnectionView;

    /// The reactive edit/create form state, one signal per field. The flat
    /// [`ConnForm`] DTO is splatted in on open ([`Self::load`]) and read back on
    /// submit ([`Self::snapshot`]), keeping the validation/mapping in the pure,
    /// tested [`ConnForm`] rather than the view.
    #[derive(Clone, Copy)]
    struct FormState {
        /// `true` ⇒ the form (not the list) is shown.
        open: RwSignal<bool>,
        editing_id: RwSignal<Option<String>>,
        kind: RwSignal<ConnectorKind>,
        id: RwSignal<String>,
        base_url: RwSignal<String>,
        api_key_env: RwSignal<String>,
        aws_profile: RwSignal<String>,
        region: RwSignal<String>,
        secret: RwSignal<String>,
        aws_access_key_id: RwSignal<String>,
        aws_secret_access_key: RwSignal<String>,
        aws_session_token: RwSignal<String>,
        clear_secret: RwSignal<bool>,
        preserved: RwSignal<PreservedFields>,
        /// Whether the connection being edited already has a stored credential
        /// (display-only — the secret itself is never fetched).
        has_credentials: RwSignal<bool>,
        /// The inline result of the per-connection "Refresh models" action
        /// (item 3), or `None` before it's used. Reset on every form open.
        refresh_status: RwSignal<Option<String>>,
        error: RwSignal<Option<String>>,
    }

    impl FormState {
        fn new() -> Self {
            Self {
                open: RwSignal::new(false),
                editing_id: RwSignal::new(None),
                kind: RwSignal::new(ConnectorKind::Anthropic),
                id: RwSignal::new(String::new()),
                base_url: RwSignal::new(String::new()),
                api_key_env: RwSignal::new(String::new()),
                aws_profile: RwSignal::new(String::new()),
                region: RwSignal::new(String::new()),
                secret: RwSignal::new(String::new()),
                aws_access_key_id: RwSignal::new(String::new()),
                aws_secret_access_key: RwSignal::new(String::new()),
                aws_session_token: RwSignal::new(String::new()),
                clear_secret: RwSignal::new(false),
                preserved: RwSignal::new(PreservedFields::default()),
                has_credentials: RwSignal::new(false),
                refresh_status: RwSignal::new(None),
                error: RwSignal::new(None),
            }
        }

        /// Splat a form + its credential state into the signals and open it.
        fn load(&self, f: ConnForm, has_credentials: bool) {
            self.editing_id.set(f.editing_id);
            self.kind.set(f.kind);
            self.id.set(f.id);
            self.base_url.set(f.base_url);
            self.api_key_env.set(f.api_key_env);
            self.aws_profile.set(f.aws_profile);
            self.region.set(f.region);
            self.secret.set(f.secret);
            self.aws_access_key_id.set(f.aws_access_key_id);
            self.aws_secret_access_key.set(f.aws_secret_access_key);
            self.aws_session_token.set(f.aws_session_token);
            self.clear_secret.set(f.clear_secret);
            self.preserved.set(f.preserved);
            self.has_credentials.set(has_credentials);
            self.refresh_status.set(None);
            self.error.set(None);
            self.open.set(true);
        }

        fn open_create(&self) {
            self.load(ConnForm::blank(ConnectorKind::Anthropic), false);
        }

        fn close(&self) {
            self.open.set(false);
            self.error.set(None);
        }

        /// Read the signals back into a pure [`ConnForm`] for validation/build.
        fn snapshot(&self) -> ConnForm {
            ConnForm {
                editing_id: self.editing_id.get_untracked(),
                kind: self.kind.get_untracked(),
                id: self.id.get_untracked(),
                base_url: self.base_url.get_untracked(),
                api_key_env: self.api_key_env.get_untracked(),
                aws_profile: self.aws_profile.get_untracked(),
                region: self.region.get_untracked(),
                secret: self.secret.get_untracked(),
                aws_access_key_id: self.aws_access_key_id.get_untracked(),
                aws_secret_access_key: self.aws_secret_access_key.get_untracked(),
                aws_session_token: self.aws_session_token.get_untracked(),
                clear_secret: self.clear_secret.get_untracked(),
                preserved: self.preserved.get_untracked(),
            }
        }
    }

    /// Panel-local delete-confirmation state. `force_offered` flips on once the
    /// daemon refuses a non-force delete (a referenced connection), revealing the
    /// force retry.
    #[derive(Clone)]
    struct Confirm {
        id: String,
        force_offered: bool,
        error: Option<String>,
    }

    /// The Connections settings panel: a live list of connections with add /
    /// configure / delete + credential entry, or the edit form when one is open.
    pub fn connections_panel(engine: EngineHandle, view: ViewSignals) -> impl IntoView {
        let form = FormState::new();
        let confirm = RwSignal::new(None::<Confirm>);

        // Load the list once the panel mounts (re-created each time the tab is
        // opened, so this refreshes on every open). Deferred via an effect so the
        // signal writes don't happen during render.
        Effect::new(move |_| {
            engine.with_value(|e| e.borrow().refresh_connections());
        });

        view! {
            <section class="panel connections-panel">
                {move || {
                    if form.open.get() {
                        connection_form(engine, view, form).into_any()
                    } else {
                        connection_list(engine, view, form, confirm).into_any()
                    }
                }}
            </section>
        }
    }

    /// The list view: intro, error banner, an optional delete-confirm, the cards
    /// (or loading/empty state), and the "Add connection" button.
    fn connection_list(
        engine: EngineHandle,
        view: ViewSignals,
        form: FormState,
        confirm: RwSignal<Option<Confirm>>,
    ) -> impl IntoView {
        view! {
            <div class="conn-list">
                <p class="panel-note muted">
                    "LLM provider connections. Add, edit, or remove connections and set their credentials."
                </p>

                <Show when=move || view.connections_error.get().is_some()>
                    <p class="conn-error" role="alert">
                        {move || view.connections_error.get().unwrap_or_default()}
                    </p>
                </Show>

                {move || confirm.get().map(|c| confirm_block(engine, confirm, c))}

                {move || {
                    if view.connections_busy.get() && !view.connections_loaded.get() {
                        view! { <p class="empty muted">"Loading connections…"</p> }.into_any()
                    } else {
                        let conns = view.connections.get();
                        if conns.is_empty() {
                            view! {
                                <p class="empty muted">"No connections yet. Add one below."</p>
                            }
                                .into_any()
                        } else {
                            conns
                                .into_iter()
                                .map(|c| card(form, confirm, c))
                                .collect_view()
                                .into_any()
                        }
                    }
                }}

                <button
                    class="conn-btn primary conn-add"
                    on:click=move |_| form.open_create()
                >
                    "+ Add connection"
                </button>
            </div>
        }
    }

    /// One connection card: status dot, id + type, availability + credential
    /// state, and Configure / Delete actions.
    fn card(
        form: FormState,
        confirm: RwSignal<Option<Confirm>>,
        c: ConnectionView,
    ) -> impl IntoView {
        let is_ok = availability_is_ok(&c.availability);
        let subtitle = format!(
            "{} · {}",
            availability_label(&c.availability),
            credentials_label(c.has_credentials)
        );
        let title = format!("{}  ({})", c.id, c.connector_type);
        let for_edit = c.clone();
        let delete_id = c.id.clone();

        view! {
            <div class="conn-card">
                <span class=if is_ok { "conn-dot ok" } else { "conn-dot bad" }></span>
                <div class="conn-card-main">
                    <span class="conn-card-title">{title}</span>
                    <span class="conn-card-sub muted">{subtitle}</span>
                </div>
                <div class="conn-actions">
                    <button
                        class="conn-btn"
                        on:click=move |_| {
                            form.load(ConnForm::from_view(&for_edit), for_edit.has_credentials);
                        }
                    >
                        "Configure"
                    </button>
                    <button
                        class="conn-btn danger"
                        on:click=move |_| {
                            confirm
                                .set(
                                    Some(Confirm {
                                        id: delete_id.clone(),
                                        force_offered: false,
                                        error: None,
                                    }),
                                )
                        }
                    >
                        "Delete"
                    </button>
                </div>
            </div>
        }
    }

    /// The delete-confirmation block. Offers a force retry once a plain delete is
    /// refused because a purpose still references the connection.
    fn confirm_block(
        engine: EngineHandle,
        confirm: RwSignal<Option<Confirm>>,
        c: Confirm,
    ) -> impl IntoView {
        // `Copy` (captures are all `Copy`), so both Delete and Force buttons can
        // reuse it. Re-reads the live confirm state so the retry keeps the id.
        let start_delete = move |force: bool| {
            let Some(cur) = confirm.get_untracked() else {
                return;
            };
            let id = cur.id.clone();
            let id_done = id.clone();
            let was_offered = cur.force_offered;
            let done: ActionDone = Rc::new(move |res: Result<(), String>| match res {
                Ok(()) => confirm.set(None),
                Err(e) => {
                    // Reveal the force retry when the daemon refuses because a
                    // purpose references the connection.
                    let offer = was_offered || (!force && e.to_lowercase().contains("purpose"));
                    confirm.set(Some(Confirm {
                        id: id_done.clone(),
                        force_offered: offer,
                        error: Some(e),
                    }));
                }
            });
            engine.with_value(|e| e.borrow().delete_connection(id, force, done));
        };

        let force_offered = c.force_offered;
        let error = c.error.clone();
        let heading = format!("Delete \u{201c}{}\u{201d}?", c.id);

        view! {
            <div class="conn-confirm" role="alertdialog">
                <p class="conn-confirm-q">{heading}</p>
                {error.map(|e| view! { <p class="conn-error">{e}</p> })}
                <Show when=move || force_offered>
                    <p class="conn-note muted">
                        "Purposes using this connection will fall back to the interactive purpose."
                    </p>
                </Show>
                <div class="conn-form-actions">
                    <button class="conn-btn" on:click=move |_| confirm.set(None)>
                        "Cancel"
                    </button>
                    {force_offered
                        .then(|| {
                            view! {
                                <button
                                    class="conn-btn danger"
                                    on:click=move |_| start_delete(true)
                                >
                                    "Force delete"
                                </button>
                            }
                        })}
                    <button class="conn-btn danger" on:click=move |_| start_delete(false)>
                        "Delete"
                    </button>
                </div>
            </div>
        }
    }

    /// The create/edit form.
    fn connection_form(engine: EngineHandle, view: ViewSignals, form: FormState) -> impl IntoView {
        let on_save = move |_| match form.snapshot().build() {
            Ok(built) => {
                form.error.set(None);
                let done: ActionDone = Rc::new(move |res: Result<(), String>| match res {
                    Ok(()) => form.close(),
                    Err(e) => form.error.set(Some(e)),
                });
                engine.with_value(|e| {
                    e.borrow().save_connection(
                        built.editing_id,
                        built.id,
                        built.config,
                        built.credential,
                        done,
                    )
                });
            }
            Err(e) => form.error.set(Some(e)),
        };

        view! {
            <div class="conn-form">
                <div class="conn-form-head">
                    <button class="conn-btn" on:click=move |_| form.close()>
                        "\u{2039} Back"
                    </button>
                    <h3 class="conn-form-title">
                        {move || match form.editing_id.get() {
                            Some(id) => format!("Edit {id}"),
                            None => "New connection".to_string(),
                        }}
                    </h3>
                </div>

                // Type: chips on create, a locked label on edit.
                <div class="field">
                    <span class="field-label">"Type"</span>
                    {move || {
                        if form.editing_id.get().is_some() {
                            view! {
                                <p class="conn-locked">
                                    {form.kind.get().label()} " · locked on edit"
                                </p>
                            }
                                .into_any()
                        } else {
                            view! {
                                <div class="segmented" role="group" aria-label="Connector type">
                                    {ConnectorKind::ALL
                                        .iter()
                                        .copied()
                                        .map(|k| {
                                            let active = move || form.kind.get() == k;
                                            view! {
                                                <button
                                                    class="segment"
                                                    class:active=active
                                                    aria-pressed=move || {
                                                        if active() { "true" } else { "false" }
                                                    }
                                                    on:click=move |_| form.kind.set(k)
                                                >
                                                    {k.label()}
                                                </button>
                                            }
                                        })
                                        .collect_view()}
                                </div>
                            }
                                .into_any()
                        }
                    }}
                </div>

                // Id: editable slug on create, locked on edit.
                {move || {
                    let editing = form.editing_id.get().is_some();
                    view! {
                        <div class="field">
                            <label class="field-label">
                                {if editing { "Connection id (locked)" } else { "Connection id (slug)" }}
                            </label>
                            <input
                                class="conn-input"
                                type="text"
                                autocomplete="off"
                                placeholder="e.g. work, aws-prod, local"
                                disabled=editing
                                prop:value=move || form.id.get()
                                on:input=move |ev| form.id.set(event_target_value(&ev))
                            />
                        </div>
                    }
                }}

                // Per-connector config fields.
                {move || kind_fields(form)}

                // Credential entry (hidden for connectors that take none).
                {move || credential_section(form)}

                // Per-connection "Refresh models" (edit only — a connection must
                // exist to refresh it). All connectors can refresh; Bedrock (which
                // caches its model list) is the motivating case.
                {move || {
                    if form.editing_id.get().is_some() {
                        refresh_models_section(engine, form).into_any()
                    } else {
                        ().into_any()
                    }
                }}

                <Show when=move || form.error.get().is_some()>
                    <p class="conn-error" role="alert">
                        {move || form.error.get().unwrap_or_default()}
                    </p>
                </Show>

                <div class="conn-form-actions">
                    <button class="conn-btn" on:click=move |_| form.close()>
                        "Cancel"
                    </button>
                    <button
                        class="conn-btn primary"
                        disabled=move || view.connections_busy.get()
                        on:click=on_save
                    >
                        "Save"
                    </button>
                </div>
            </div>
        }
    }

    /// The per-connector config inputs, keyed on the selected kind.
    fn kind_fields(form: FormState) -> AnyView {
        match form.kind.get() {
            ConnectorKind::Anthropic | ConnectorKind::OpenAi => view! {
                {text_field("Base URL (optional)", form.base_url, "https://api.example.com/v1")}
                {text_field("API key env var (optional)", form.api_key_env, "e.g. OPENAI_API_KEY")}
            }
            .into_any(),
            ConnectorKind::Bedrock => view! {
                {text_field("AWS profile (optional)", form.aws_profile, "default")}
                {text_field("Region", form.region, "us-west-2")}
                {text_field("Base URL (optional)", form.base_url, "")}
            }
            .into_any(),
            ConnectorKind::Ollama => {
                text_field("Base URL", form.base_url, "http://localhost:11434").into_any()
            }
        }
    }

    /// A labelled single-line text input bound to `value`.
    fn text_field(
        label: &'static str,
        value: RwSignal<String>,
        placeholder: &'static str,
    ) -> impl IntoView {
        view! {
            <div class="field">
                <label class="field-label">{label}</label>
                <input
                    class="conn-input"
                    type="text"
                    autocomplete="off"
                    placeholder=placeholder
                    prop:value=move || value.get()
                    on:input=move |ev| value.set(event_target_value(&ev))
                />
            </div>
        }
    }

    /// The write-only credential entry: current state, the credential
    /// input(s) — a single API-key field, or Bedrock's three separate fields
    /// (Access Key ID / Secret Access Key / optional Session Token) — an
    /// explicit "clear" toggle (only when one is stored), and the posture note.
    /// Renders nothing for connectors that take no credential (Ollama).
    fn credential_section(form: FormState) -> AnyView {
        let kind = form.kind.get();
        if !kind.accepts_credential() {
            return ().into_any();
        }
        view! {
            <div class="field conn-cred">
                <span class="field-label">"Credential"</span>
                <p class="conn-note muted">
                    {move || credentials_label(form.has_credentials.get())}
                </p>
                {credential_inputs(kind, form)}
                <Show when=move || form.has_credentials.get()>
                    <label class="conn-check">
                        <input
                            type="checkbox"
                            prop:checked=move || form.clear_secret.get()
                            on:change=move |ev| form.clear_secret.set(event_target_checked(&ev))
                        />
                        "Clear the stored credential"
                    </label>
                </Show>
                <p class="conn-note muted">
                    "Sent write-only to the daemon over your private tailnet; never shown here. Leave blank to keep the current credential."
                </p>
            </div>
        }
        .into_any()
    }

    /// The credential input control(s) for `kind`: Bedrock gets three separate
    /// write-only fields (joined on save into `ACCESS_KEY_ID:SECRET[:SESSION]`);
    /// the api-key connectors get one raw-key field.
    fn credential_inputs(kind: ConnectorKind, form: FormState) -> AnyView {
        match kind {
            ConnectorKind::Bedrock => view! {
                {cred_field("Access Key ID", form.aws_access_key_id, "AKIA\u{2026}", false)}
                {cred_field("Secret Access Key", form.aws_secret_access_key, "Secret access key", true)}
                {cred_field(
                    "Session Token (optional)",
                    form.aws_session_token,
                    "Only for temporary credentials",
                    true,
                )}
            }
            .into_any(),
            _ => view! {
                <input
                    class="conn-input"
                    type="password"
                    autocomplete="off"
                    placeholder=kind.credential_placeholder()
                    prop:value=move || form.secret.get()
                    on:input=move |ev| form.secret.set(event_target_value(&ev))
                />
            }
            .into_any(),
        }
    }

    /// One labelled write-only credential input (Bedrock's separate fields).
    /// `password` masks the value while typing (the secret + session token);
    /// the Access Key ID is a plain identifier, so it stays legible.
    fn cred_field(
        label: &'static str,
        value: RwSignal<String>,
        placeholder: &'static str,
        password: bool,
    ) -> impl IntoView {
        view! {
            <div class="conn-cred-field">
                <label class="sub-label">{label}</label>
                <input
                    class="conn-input"
                    type=if password { "password" } else { "text" }
                    autocomplete="off"
                    placeholder=placeholder
                    prop:value=move || value.get()
                    on:input=move |ev| value.set(event_target_value(&ev))
                />
            </div>
        }
    }

    /// The per-connection "Refresh models" action (item 3), shown only when
    /// editing an existing connection. Triggers a cache-bypassing
    /// `ListAvailableModels { connection_id, refresh: true }` and shows the
    /// resulting model count (or an error) inline. Mirrors the KCM's Bedrock
    /// "Refresh models" button, but offered for every connector.
    fn refresh_models_section(engine: EngineHandle, form: FormState) -> impl IntoView {
        let status = form.refresh_status;
        let on_refresh = move |_| {
            let Some(id) = form.editing_id.get_untracked() else {
                return;
            };
            status.set(Some("Refreshing\u{2026}".to_string()));
            let done: ModelsRefreshed = Rc::new(move |res: Result<usize, String>| match res {
                Ok(count) => status.set(Some(format!(
                    "{count} model{} available",
                    if count == 1 { "" } else { "s" }
                ))),
                Err(e) => status.set(Some(format!("Refresh failed: {e}"))),
            });
            engine.with_value(|e| e.borrow().refresh_connection_models(id, done));
        };
        view! {
            <div class="field conn-refresh">
                <span class="field-label">"Models"</span>
                <button class="conn-btn conn-refresh-btn" on:click=on_refresh>
                    "\u{21bb} Refresh models"
                </button>
                <Show when=move || status.get().is_some()>
                    <p class="conn-note muted conn-refresh-status" role="status">
                        {move || status.get().unwrap_or_default()}
                    </p>
                </Show>
            </div>
        }
    }
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

    // --- join_bedrock_credential (item 2: separate Bedrock fields) ------------

    #[test]
    fn join_bedrock_two_parts_has_no_trailing_colon() {
        // Access key id + secret, no session token: exactly `ACCESS:SECRET`.
        assert_eq!(
            join_bedrock_credential("AKIAEXAMPLE", "wJalr/secret", ""),
            Some("AKIAEXAMPLE:wJalr/secret".to_string())
        );
    }

    #[test]
    fn join_bedrock_three_parts_appends_session_token() {
        assert_eq!(
            join_bedrock_credential("AKIAEXAMPLE", "wJalr/secret", "FwoGZXIvYXdz//SESSION"),
            Some("AKIAEXAMPLE:wJalr/secret:FwoGZXIvYXdz//SESSION".to_string())
        );
    }

    #[test]
    fn join_bedrock_trims_each_field() {
        // Surrounding whitespace (e.g. a pasted key with a trailing newline) is
        // stripped from every part before joining.
        assert_eq!(
            join_bedrock_credential("  AKIAEXAMPLE\n", "\twJalr/secret ", "  TOKEN\n"),
            Some("AKIAEXAMPLE:wJalr/secret:TOKEN".to_string())
        );
    }

    #[test]
    fn join_bedrock_all_blank_is_none() {
        assert_eq!(join_bedrock_credential("", "", ""), None);
        assert_eq!(join_bedrock_credential("  ", "\t", " \n "), None);
    }

    #[test]
    fn join_bedrock_requires_both_access_and_secret() {
        // A partial entry (only one of the required pair) is "no credential",
        // never a malformed half-string like `AKIA:` or `:secret`.
        assert_eq!(join_bedrock_credential("AKIAEXAMPLE", "", ""), None);
        assert_eq!(join_bedrock_credential("", "wJalr/secret", ""), None);
        // A session token alone can never stand in for the required pair.
        assert_eq!(join_bedrock_credential("", "", "TOKEN"), None);
    }

    #[test]
    fn join_bedrock_blank_session_is_dropped() {
        // Whitespace-only session token collapses to the two-part form.
        assert_eq!(
            join_bedrock_credential("AKIAEXAMPLE", "wJalr/secret", "   "),
            Some("AKIAEXAMPLE:wJalr/secret".to_string())
        );
    }

    // --- bedrock_credential_action --------------------------------------------

    #[test]
    fn bedrock_credential_action_sets_joined_value() {
        assert_eq!(
            bedrock_credential_action("AKIAEXAMPLE", "wJalr/secret", "", false),
            Some(CredentialAction::Set(
                "AKIAEXAMPLE:wJalr/secret".to_string()
            ))
        );
    }

    #[test]
    fn bedrock_credential_action_clear_wins_over_typed_fields() {
        // An explicit clear removes the secret even with stray field text.
        assert_eq!(
            bedrock_credential_action("AKIAEXAMPLE", "wJalr/secret", "TOKEN", true),
            Some(CredentialAction::Clear)
        );
    }

    #[test]
    fn bedrock_credential_action_blank_no_clear_is_none() {
        // The write-only no-op: blank fields must never wipe a stored secret.
        assert_eq!(bedrock_credential_action("", "", "", false), None);
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
    fn build_bedrock_joins_separate_credential_fields() {
        // The three write-only inputs join into the daemon's credential string.
        let mut form = ConnForm::blank(ConnectorKind::Bedrock);
        form.id = "aws".into();
        form.aws_access_key_id = "AKIAEXAMPLE".into();
        form.aws_secret_access_key = "wJalr/secret".into();
        let built = form.build().expect("valid form builds");
        assert_eq!(
            built.credential,
            Some(CredentialAction::Set("AKIAEXAMPLE:wJalr/secret".into()))
        );
        // The single `secret` field is irrelevant for Bedrock.
        let mut form2 = form.clone();
        form2.aws_session_token = "SESSION-TOKEN".into();
        let built2 = form2.build().expect("valid form builds");
        assert_eq!(
            built2.credential,
            Some(CredentialAction::Set(
                "AKIAEXAMPLE:wJalr/secret:SESSION-TOKEN".into()
            ))
        );
    }

    #[test]
    fn build_bedrock_blank_credential_fields_are_no_change() {
        // No credential fields typed and no clear: the stored secret is untouched.
        let mut form = ConnForm::blank(ConnectorKind::Bedrock);
        form.id = "aws".into();
        let built = form.build().expect("valid form builds");
        assert_eq!(built.credential, None);
    }

    #[test]
    fn build_bedrock_clear_credential() {
        let mut form = ConnForm::blank(ConnectorKind::Bedrock);
        form.editing_id = Some("aws".into());
        form.clear_secret = true;
        let built = form.build().expect("valid form builds");
        assert_eq!(built.credential, Some(CredentialAction::Clear));
    }

    #[test]
    fn from_view_never_prefills_bedrock_credential_fields() {
        // has_credentials is true, but the three credential inputs stay empty and
        // no clear is staged — a stored secret is never echoed or round-tripped.
        let config = ConnectionConfigView::Bedrock {
            aws_profile: Some("prod".into()),
            region: Some("us-west-2".into()),
            base_url: None,
            connect_timeout_secs: None,
            stream_timeout_secs: None,
            max_context_tokens: None,
        };
        let form = ConnForm::from_view(&view("aws", "bedrock", Some(config)));
        assert_eq!(form.aws_access_key_id, "");
        assert_eq!(form.aws_secret_access_key, "");
        assert_eq!(form.aws_session_token, "");
        assert!(!form.clear_secret);
        // The build carries no credential when the fields are left blank.
        assert_eq!(form.build().expect("valid").credential, None);
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
