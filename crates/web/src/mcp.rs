//! The MCP-servers settings panel (issue #55): manage the daemon's Model
//! Context Protocol servers — list them with an honest status, enable/disable,
//! add/edit (local stdio or remote HTTP), remove, and set a remote server's
//! bearer token or point it at an OAuth service account.
//!
//! **Pure additive client panel.** Every command this panel issues already
//! exists on the typed `Command`/`CommandResult` surface (`ListMcpServers`,
//! `SetMcpServerEnabled`, `RemoveMcpServer`, `UpsertMcpServer`, `SetMcpSecret`,
//! `ListServiceAccounts`) and the BFF blind-forwards it — no BFF/daemon/protocol
//! change. The transport-aware add/edit rides `UpsertMcpServer { config_json }`,
//! a JSON string of the daemon's `McpServerConfig`; this module builds that JSON
//! from a small local DTO ([`McpConfigDto`]) mirroring only the fields the form
//! surfaces, so the web crate never pulls the process-spawning
//! `desktop-assistant-mcp-client` (it would not stay wasm-clean).
//!
//! **Split like [`crate::connections`] / [`crate::tasks`].** The pure form ⇄
//! `config_json` mapping, the status/transport display vocabulary, and the
//! env/args/scope parsers are transport-/view-free, so they compile and
//! unit-test on the host target. The Leptos view (`#[cfg(target_arch =
//! "wasm32")]`) and the engine commands are the thin wasm shell over that logic.
//!
//! **Bearer secrets are write-only.** A bearer token is never echoed by the
//! daemon (the view carries only refs/kinds), never pre-filled on edit, and only
//! sent — via [`Command::SetMcpSecret`] under the `{name}_token` ref, *before*
//! the `UpsertMcpServer` that references it — when the user actually types one.
//! OAuth carries only the service-account *ref*; secret values never leave via
//! this panel.
//!
//! **Honest OAuth degradation.** Interactive OAuth sign-in
//! (`configure_command`) spawns a browser *on the daemon host*; a phone browser
//! over Tailscale cannot drive it and there is no web OAuth-launch path. So an
//! OAuth server that is not yet authorized renders honestly (`Sign in required`)
//! with an informational note pointing at the desktop settings — never a
//! non-functional sign-in button. A web-drivable OAuth flow is a follow-up.

use std::collections::BTreeMap;

use desktop_assistant_api_model::{Command, McpServerView, Secret};

// ===========================================================================
// Pure logic (host-testable)
// ===========================================================================

/// A minimal `#[derive(Serialize)]` mirror of the daemon's `McpServerConfig`,
/// carrying only the fields the web form surfaces. Building the `config_json`
/// from this DTO (rather than depending on `desktop-assistant-mcp-client`) keeps
/// the wasm crate free of that crate's process-spawn / native code. The daemon's
/// `McpServerConfig` uses serde defaults for every field this omits, so
/// omit-empty is safe; `env` is a `BTreeMap` so its JSON is key-sorted and the
/// wire form is deterministic (a `HashMap` would reorder between builds).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
struct McpConfigDto {
    name: String,
    enabled: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    command: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    env: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    http: Option<HttpDto>,
}

/// The `http` sub-table of [`McpConfigDto`] — mirrors the daemon's
/// `HttpTransportConfig` for the two auth modes the web form drives: a static
/// bearer token (by secret ref) or a reference to an OAuth service account
/// (epic #477). The inline `oauth` block is intentionally not surfaced here.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
struct HttpDto {
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    auth_bearer_secret: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    oauth_account: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    scopes: Vec<String>,
}

/// The transport a server speaks. Selects which set of form fields is shown and
/// which shape [`McpForm::build`] emits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpTransport {
    /// Local process spawned over stdio (`command`/`args`/`env`).
    Stdio,
    /// Remote streamable-HTTP endpoint (`url` + auth).
    Http,
}

/// How a remote (HTTP) server authenticates. Mirrors the daemon's `auth_kind`
/// (`"none"` | `"bearer"` | `"oauth"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpAuthKind {
    /// No authentication.
    None,
    /// A static `Authorization: Bearer` token, stored write-only under the
    /// `{name}_token` secret ref.
    Bearer,
    /// OAuth 2.0 via a reusable service account (epic #477) referenced by id.
    OAuth,
}

/// Map the coarse daemon status string to a `(dot CSS class, human label)`
/// pair. Covers the six states the daemon reports; any unrecognized future
/// state renders as a neutral "Unknown" rather than panicking, so an older
/// client degrades honestly against a newer daemon.
pub fn status_display(status: &str) -> (&'static str, &'static str) {
    match status {
        "running" => ("mcp-dot ok", "Running"),
        "stopped" => ("mcp-dot neutral", "Stopped"),
        "disabled" => ("mcp-dot neutral", "Disabled"),
        "needs_auth" => ("mcp-dot warn", "Sign in required"),
        "auth_expired" => ("mcp-dot warn", "Sign in expired"),
        "error" => ("mcp-dot error", "Error"),
        _ => ("mcp-dot neutral", "Unknown"),
    }
}

/// The transport chip label: an HTTP server is `"remote"`, anything else
/// (stdio) is `"local"`.
pub fn transport_chip(transport: &str) -> &'static str {
    if transport == "http" {
        "remote"
    } else {
        "local"
    }
}

/// Parse an env textarea into ordered `(KEY, value)` pairs. Each non-blank line
/// is `KEY=value`; the key is trimmed and the value is everything after the
/// first `=` (values may themselves contain `=`), also trimmed for paste
/// hygiene. Lines without a `=`, or with a blank key, are skipped — a malformed
/// line is dropped, never turned into a half-entry.
pub fn parse_env(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let (key, value) = line.split_once('=')?;
            let key = key.trim();
            if key.is_empty() {
                return None;
            }
            Some((key.to_string(), value.trim().to_string()))
        })
        .collect()
}

/// Split a space-separated args string into argv tokens. Any run of whitespace
/// separates; empty tokens are dropped. Deliberately simple — a server needing
/// shell-quoted args with embedded spaces is a rare case the v1 form leaves to a
/// direct config edit.
pub fn split_args(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_string).collect()
}

/// Split an OAuth scopes string on whitespace and/or commas into individual
/// scopes, dropping empties.
pub fn split_scopes(text: &str) -> Vec<String> {
    text.split([',', ' ', '\t', '\n', '\r'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// The secrets.toml ref a server's bearer token is stored under. Convention:
/// `{name}_token`, so a server's config can reference its token by a stable id
/// the user never has to hand-edit.
pub fn bearer_secret_ref(name: &str) -> String {
    format!("{name}_token")
}

/// Build the [`Command::SetMcpSecret`] that stores a bearer token value under
/// `id`. The value is wrapped in [`desktop_assistant_api_model::Secret`] so it
/// can't leak via `Debug`.
pub fn mcp_secret_command(id: String, value: String) -> Command {
    Command::SetMcpSecret {
        id,
        value: Secret(value),
    }
}

/// Validate a server name on create: non-empty and only letters, digits, `-`,
/// `_` (mirrors [`crate::connections`]'s slug contract — the name is a config
/// table key and a tool-namespace prefix).
fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Server name is required.".to_string());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("Name may only contain letters, digits, '-', and '_'.".to_string());
    }
    Ok(())
}

/// Trim `s`; `None` when the trimmed result is empty (so an empty optional is
/// omitted from the JSON rather than sent as `""`).
fn opt(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// The reactive-free model of the add/edit form. The flat DTO is splatted into
/// the view signals on open and read back on submit, keeping the
/// validation/mapping here (tested) rather than in the view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpForm {
    /// `true` when editing an existing server — the name is immutable.
    pub editing: bool,
    pub transport: McpTransport,
    pub name: String,
    pub enabled: bool,
    // --- stdio ---
    pub command: String,
    /// Space-separated argv (split on save).
    pub args: String,
    pub namespace: String,
    /// `KEY=value` lines (parsed on save).
    pub env: String,
    // --- http ---
    pub url: String,
    pub auth: McpAuthKind,
    /// Write-only bearer token; never populated from a view.
    pub bearer_token: String,
    /// Referenced service-account id (OAuth).
    pub oauth_account: String,
    /// Space/comma-separated OAuth scopes.
    pub scopes: String,
}

impl McpForm {
    /// A blank create form for `transport`.
    pub fn blank(transport: McpTransport) -> Self {
        Self {
            editing: false,
            transport,
            name: String::new(),
            enabled: true,
            command: String::new(),
            args: String::new(),
            namespace: String::new(),
            env: String::new(),
            url: String::new(),
            auth: McpAuthKind::None,
            bearer_token: String::new(),
            oauth_account: String::new(),
            scopes: String::new(),
        }
    }

    /// Pre-fill an edit form from a server view: name + transport, the surfaced
    /// non-secret config fields, and (for http) the auth kind + oauth ref/scopes.
    /// Secret material (the bearer token) stays blank — the daemon never echoes
    /// it. The `env` box also stays blank: the view does not carry env, so
    /// editing a stdio server cannot pre-fill it (see the form note).
    pub fn from_view(view: &McpServerView) -> Self {
        let transport = if view.transport == "http" {
            McpTransport::Http
        } else {
            McpTransport::Stdio
        };
        let auth = match view.auth_kind.as_deref() {
            Some("bearer") => McpAuthKind::Bearer,
            Some("oauth") => McpAuthKind::OAuth,
            _ => McpAuthKind::None,
        };
        // For http the target is the url; for stdio the command is authoritative.
        let url = if transport == McpTransport::Http {
            view.target.clone()
        } else {
            String::new()
        };
        Self {
            editing: true,
            transport,
            name: view.name.clone(),
            enabled: view.enabled,
            command: view.command.clone(),
            args: view.args.join(" "),
            namespace: view.namespace.clone().unwrap_or_default(),
            // The view carries no env — it can't be pre-filled (see the form note).
            env: String::new(),
            url,
            auth,
            // Write-only: the bearer token is never echoed / pre-filled.
            bearer_token: String::new(),
            oauth_account: view.oauth_account_ref.clone().unwrap_or_default(),
            scopes: view.oauth_scopes.join(" "),
        }
    }

    /// Validate + assemble the form into the command inputs: the target name
    /// (typed + validated on create, immutable on edit), the `config_json`
    /// string [`Command::UpsertMcpServer`] receives, and the optional bearer
    /// secret `(ref, value)` to write *first*. `Err` carries a human-readable
    /// reason.
    pub fn build(&self) -> Result<BuiltMcpServer, String> {
        let name = self.name.trim().to_string();
        // The name is immutable on edit (already daemon-validated); only a
        // freshly-typed create name is checked.
        if !self.editing {
            validate_name(&name)?;
        }

        let (dto, secret) = match self.transport {
            McpTransport::Stdio => {
                let command = self.command.trim().to_string();
                if command.is_empty() {
                    return Err("Command is required for a local (stdio) server.".to_string());
                }
                let dto = McpConfigDto {
                    name: name.clone(),
                    enabled: self.enabled,
                    command,
                    args: split_args(&self.args),
                    namespace: opt(&self.namespace),
                    env: parse_env(&self.env).into_iter().collect(),
                    http: None,
                };
                (dto, None)
            }
            McpTransport::Http => {
                let url = self.url.trim().to_string();
                if url.is_empty() {
                    return Err("URL is required for a remote (HTTP) server.".to_string());
                }
                let (auth_bearer_secret, oauth_account, scopes, secret) = match self.auth {
                    McpAuthKind::None => (None, None, Vec::new(), None),
                    McpAuthKind::Bearer => {
                        let secret_ref = bearer_secret_ref(&name);
                        let token = self.bearer_token.trim();
                        // Write-only: only write a secret when the user typed one;
                        // a blank field leaves any stored token untouched. The
                        // config still references the ref so the server stays
                        // "bearer" rather than silently going unauthenticated.
                        let secret = if token.is_empty() {
                            None
                        } else {
                            Some((secret_ref.clone(), token.to_string()))
                        };
                        (Some(secret_ref), None, Vec::new(), secret)
                    }
                    McpAuthKind::OAuth => {
                        let account = self.oauth_account.trim().to_string();
                        if account.is_empty() {
                            return Err(
                                "Choose a service account for OAuth authentication.".to_string()
                            );
                        }
                        (None, Some(account), split_scopes(&self.scopes), None)
                    }
                };
                let dto = McpConfigDto {
                    name: name.clone(),
                    enabled: self.enabled,
                    command: String::new(),
                    args: Vec::new(),
                    namespace: None,
                    env: BTreeMap::new(),
                    http: Some(HttpDto {
                        url,
                        auth_bearer_secret,
                        oauth_account,
                        scopes,
                    }),
                };
                (dto, secret)
            }
        };

        let config_json = serde_json::to_string(&dto)
            .map_err(|e| format!("Failed to encode the server config: {e}"))?;
        Ok(BuiltMcpServer {
            editing: self.editing,
            name,
            config_json,
            secret,
        })
    }
}

/// The assembled inputs for an upsert (+ optional bearer secret) round-trip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltMcpServer {
    /// `true` ⇒ the name already existed (edit); `false` ⇒ create. Both go
    /// through `UpsertMcpServer`, which is add-or-replace.
    pub editing: bool,
    /// The target server name (immutable on edit, validated on create).
    pub name: String,
    /// The JSON `McpServerConfig` string for `UpsertMcpServer { config_json }`.
    pub config_json: String,
    /// `(secret_ref, value)` to store via `SetMcpSecret` *before* the upsert,
    /// when the user typed a bearer token. `None` leaves any stored secret
    /// untouched (write-only: a blank field never wipes a token).
    pub secret: Option<(String, String)>,
}

// ===========================================================================
// Leptos view (wasm only)
// ===========================================================================

#[cfg(target_arch = "wasm32")]
pub use view::mcp_panel;

#[cfg(target_arch = "wasm32")]
mod view {
    use std::rc::Rc;

    use leptos::prelude::*;

    use super::{McpAuthKind, McpForm, McpTransport, status_display, transport_chip};
    use crate::engine::{ActionDone, ViewSignals};
    use crate::settings::EngineHandle;
    use desktop_assistant_api_model::McpServerView;

    /// Label for the transport segmented control.
    fn transport_label(t: McpTransport) -> &'static str {
        match t {
            McpTransport::Stdio => "Local (stdio)",
            McpTransport::Http => "Remote (HTTP)",
        }
    }

    /// Label for the auth segmented control (http only).
    fn auth_label(a: McpAuthKind) -> &'static str {
        match a {
            McpAuthKind::None => "None",
            McpAuthKind::Bearer => "Bearer token",
            McpAuthKind::OAuth => "OAuth account",
        }
    }

    /// The reactive edit/create form: one signal per field. The flat [`McpForm`]
    /// DTO is splatted in on open ([`Self::load`]) and read back on submit
    /// ([`Self::snapshot`]), so the validation/mapping stays in the tested pure
    /// [`McpForm`] rather than the view.
    #[derive(Clone, Copy)]
    struct FormState {
        /// `true` ⇒ the form (not the list) is shown.
        open: RwSignal<bool>,
        editing: RwSignal<bool>,
        transport: RwSignal<McpTransport>,
        name: RwSignal<String>,
        enabled: RwSignal<bool>,
        command: RwSignal<String>,
        args: RwSignal<String>,
        namespace: RwSignal<String>,
        env: RwSignal<String>,
        url: RwSignal<String>,
        auth: RwSignal<McpAuthKind>,
        bearer_token: RwSignal<String>,
        oauth_account: RwSignal<String>,
        scopes: RwSignal<String>,
        error: RwSignal<Option<String>>,
    }

    impl FormState {
        fn new() -> Self {
            Self {
                open: RwSignal::new(false),
                editing: RwSignal::new(false),
                transport: RwSignal::new(McpTransport::Stdio),
                name: RwSignal::new(String::new()),
                enabled: RwSignal::new(true),
                command: RwSignal::new(String::new()),
                args: RwSignal::new(String::new()),
                namespace: RwSignal::new(String::new()),
                env: RwSignal::new(String::new()),
                url: RwSignal::new(String::new()),
                auth: RwSignal::new(McpAuthKind::None),
                bearer_token: RwSignal::new(String::new()),
                oauth_account: RwSignal::new(String::new()),
                scopes: RwSignal::new(String::new()),
                error: RwSignal::new(None),
            }
        }

        /// Splat a pure [`McpForm`] into the signals and open the form.
        fn load(&self, f: McpForm) {
            self.editing.set(f.editing);
            self.transport.set(f.transport);
            self.name.set(f.name);
            self.enabled.set(f.enabled);
            self.command.set(f.command);
            self.args.set(f.args);
            self.namespace.set(f.namespace);
            self.env.set(f.env);
            self.url.set(f.url);
            self.auth.set(f.auth);
            self.bearer_token.set(f.bearer_token);
            self.oauth_account.set(f.oauth_account);
            self.scopes.set(f.scopes);
            self.error.set(None);
            self.open.set(true);
        }

        fn open_create(&self) {
            self.load(McpForm::blank(McpTransport::Stdio));
        }

        fn close(&self) {
            self.open.set(false);
            self.error.set(None);
        }

        /// Read the signals back into a pure [`McpForm`] for validation/build.
        fn snapshot(&self) -> McpForm {
            McpForm {
                editing: self.editing.get_untracked(),
                transport: self.transport.get_untracked(),
                name: self.name.get_untracked(),
                enabled: self.enabled.get_untracked(),
                command: self.command.get_untracked(),
                args: self.args.get_untracked(),
                namespace: self.namespace.get_untracked(),
                env: self.env.get_untracked(),
                url: self.url.get_untracked(),
                auth: self.auth.get_untracked(),
                bearer_token: self.bearer_token.get_untracked(),
                oauth_account: self.oauth_account.get_untracked(),
                scopes: self.scopes.get_untracked(),
            }
        }
    }

    /// Panel-local remove-confirmation state.
    #[derive(Clone)]
    struct Confirm {
        name: String,
        error: Option<String>,
    }

    /// The MCP-servers settings panel: a live list of servers with an honest
    /// status + enable/add/edit/remove, or the edit form when one is open.
    pub fn mcp_panel(engine: EngineHandle, view: ViewSignals) -> impl IntoView {
        let form = FormState::new();
        let confirm = RwSignal::new(None::<Confirm>);

        // Load the list + the OAuth service accounts once the panel mounts
        // (re-created each open, so this refreshes every time). Deferred via an
        // effect so the signal writes don't happen during render.
        Effect::new(move |_| {
            engine.with_value(|e| {
                let e = e.borrow();
                e.refresh_mcp_servers();
                e.refresh_service_accounts();
            });
        });

        view! {
            <section class="panel mcp-panel">
                {move || {
                    if form.open.get() {
                        mcp_form(engine, view, form).into_any()
                    } else {
                        mcp_list(engine, view, form, confirm).into_any()
                    }
                }}
            </section>
        }
    }

    /// The list view: intro, error banner, an optional remove-confirm, the cards
    /// (or loading/empty state), and the "Add server" button.
    fn mcp_list(
        engine: EngineHandle,
        view: ViewSignals,
        form: FormState,
        confirm: RwSignal<Option<Confirm>>,
    ) -> impl IntoView {
        view! {
            <div class="mcp-list">
                <p class="panel-note muted">
                    "Model Context Protocol servers give Adele extra tools. Add, edit, enable, or remove them here."
                </p>

                <Show when=move || view.mcp_error.get().is_some()>
                    <p class="mcp-error" role="alert">
                        {move || view.mcp_error.get().unwrap_or_default()}
                    </p>
                </Show>

                {move || confirm.get().map(|c| confirm_block(engine, confirm, c))}

                {move || {
                    if view.mcp_busy.get() && !view.mcp_loaded.get() {
                        view! { <p class="empty muted">"Loading MCP servers…"</p> }.into_any()
                    } else {
                        let servers = view.mcp_servers.get();
                        if servers.is_empty() {
                            view! {
                                <p class="empty muted">"No MCP servers yet. Add one below."</p>
                            }
                                .into_any()
                        } else {
                            servers
                                .into_iter()
                                .map(|s| card(engine, view, form, confirm, s))
                                .collect_view()
                                .into_any()
                        }
                    }
                }}

                <button class="mcp-btn primary mcp-add" on:click=move |_| form.open_create()>
                    "+ Add server"
                </button>
            </div>
        }
    }

    /// One server card: status dot + label, transport chip, tool count, target,
    /// last error (if any), the honest OAuth sign-in note (if any), and the
    /// Enable/Disable, Configure, and Remove actions.
    fn card(
        engine: EngineHandle,
        view: ViewSignals,
        form: FormState,
        confirm: RwSignal<Option<Confirm>>,
        s: McpServerView,
    ) -> impl IntoView {
        let (dot_class, status_label) = status_display(&s.status);
        let chip = transport_chip(&s.transport);
        let tools = if s.status == "running" && s.tool_count > 0 {
            let n = s.tool_count;
            Some(format!(
                " \u{00b7} {n} tool{}",
                if n == 1 { "" } else { "s" }
            ))
        } else {
            None
        };
        let detail = s.detail.clone();
        // Honest OAuth degradation: an OAuth server that isn't authorized can't
        // sign in from a phone browser (the flow spawns a browser on the daemon
        // host). Point the user at the desktop settings instead of a dead button.
        let needs_desktop_signin =
            s.auth_kind.as_deref() == Some("oauth") && s.oauth_authorized == Some(false);

        let name = s.name.clone();
        let enabled = s.enabled;
        let toggle_name = name.clone();
        let toggle = move |_| {
            let done: ActionDone = Rc::new(move |res: Result<(), String>| {
                if let Err(e) = res {
                    view.mcp_error.set(Some(e));
                }
            });
            engine.with_value(|e| {
                e.borrow()
                    .set_mcp_enabled(toggle_name.clone(), !enabled, done)
            });
        };

        let for_edit = s.clone();
        let configure = move |_| form.load(McpForm::from_view(&for_edit));

        let remove_name = name.clone();
        let remove = move |_| {
            confirm.set(Some(Confirm {
                name: remove_name.clone(),
                error: None,
            }));
        };

        view! {
            <div class="mcp-card">
                <div class="mcp-card-head">
                    <span class=dot_class></span>
                    <span class="mcp-card-title">{name.clone()}</span>
                    <span class="mcp-chip">{chip}</span>
                </div>
                <div class="mcp-card-sub muted">
                    <span class="mcp-status">{status_label}{tools}</span>
                </div>
                <div class="mcp-target muted">{s.target.clone()}</div>
                {detail.map(|d| view! { <p class="mcp-detail" role="alert">{d}</p> })}
                {needs_desktop_signin
                    .then(|| {
                        view! {
                            <p class="mcp-note warn">
                                "Sign-in required. Complete OAuth sign-in from the desktop settings on the daemon host — a phone browser can't drive it."
                            </p>
                        }
                    })}
                <div class="mcp-actions">
                    <button class="mcp-btn" on:click=toggle>
                        {if enabled { "Disable" } else { "Enable" }}
                    </button>
                    <button class="mcp-btn" on:click=configure>
                        "Configure"
                    </button>
                    <button class="mcp-btn danger" on:click=remove>
                        "Remove"
                    </button>
                </div>
            </div>
        }
    }

    /// The remove-confirmation block.
    fn confirm_block(
        engine: EngineHandle,
        confirm: RwSignal<Option<Confirm>>,
        c: Confirm,
    ) -> impl IntoView {
        let start_remove = move |_| {
            let Some(cur) = confirm.get_untracked() else {
                return;
            };
            let name = cur.name.clone();
            let name_done = name.clone();
            let done: ActionDone = Rc::new(move |res: Result<(), String>| match res {
                Ok(()) => confirm.set(None),
                Err(e) => confirm.set(Some(Confirm {
                    name: name_done.clone(),
                    error: Some(e),
                })),
            });
            engine.with_value(|e| e.borrow().remove_mcp_server(name, done));
        };

        let error = c.error.clone();
        let heading = format!("Remove \u{201c}{}\u{201d}?", c.name);

        view! {
            <div class="mcp-confirm" role="alertdialog">
                <p class="mcp-confirm-q">{heading}</p>
                {error.map(|e| view! { <p class="mcp-error">{e}</p> })}
                <div class="mcp-form-actions">
                    <button class="mcp-btn" on:click=move |_| confirm.set(None)>
                        "Cancel"
                    </button>
                    <button class="mcp-btn danger" on:click=start_remove>
                        "Remove"
                    </button>
                </div>
            </div>
        }
    }

    /// The create/edit form.
    fn mcp_form(engine: EngineHandle, view: ViewSignals, form: FormState) -> impl IntoView {
        let on_save = move |_| match form.snapshot().build() {
            Ok(built) => {
                form.error.set(None);
                let done: ActionDone = Rc::new(move |res: Result<(), String>| match res {
                    Ok(()) => form.close(),
                    Err(e) => form.error.set(Some(e)),
                });
                engine.with_value(|e| {
                    e.borrow()
                        .save_mcp_server(built.config_json, built.secret, done)
                });
            }
            Err(e) => form.error.set(Some(e)),
        };

        view! {
            <div class="mcp-form">
                <div class="mcp-form-head">
                    <button class="mcp-btn" on:click=move |_| form.close()>
                        "\u{2039} Back"
                    </button>
                    <h3 class="mcp-form-title">
                        {move || {
                            if form.editing.get() {
                                format!("Edit {}", form.name.get())
                            } else {
                                "New MCP server".to_string()
                            }
                        }}
                    </h3>
                </div>

                // Transport: segmented on create, locked on edit.
                <div class="field">
                    <span class="field-label">"Transport"</span>
                    {move || {
                        if form.editing.get() {
                            view! {
                                <p class="mcp-locked">
                                    {transport_label(form.transport.get())} " · locked on edit"
                                </p>
                            }
                                .into_any()
                        } else {
                            view! {
                                <div class="segmented" role="group" aria-label="Transport">
                                    {[McpTransport::Stdio, McpTransport::Http]
                                        .into_iter()
                                        .map(|t| {
                                            let active = move || form.transport.get() == t;
                                            view! {
                                                <button
                                                    class="segment"
                                                    class:active=active
                                                    aria-pressed=move || {
                                                        if active() { "true" } else { "false" }
                                                    }
                                                    on:click=move |_| form.transport.set(t)
                                                >
                                                    {transport_label(t)}
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

                // Name: editable slug on create, locked on edit.
                {move || {
                    let editing = form.editing.get();
                    view! {
                        <div class="field">
                            <label class="field-label">
                                {if editing { "Name (locked)" } else { "Name" }}
                            </label>
                            <input
                                class="mcp-input"
                                type="text"
                                autocomplete="off"
                                placeholder="e.g. files, gmail, github"
                                disabled=editing
                                prop:value=move || form.name.get()
                                on:input=move |ev| form.name.set(event_target_value(&ev))
                            />
                        </div>
                    }
                }}

                <label class="mcp-check">
                    <input
                        type="checkbox"
                        prop:checked=move || form.enabled.get()
                        on:change=move |ev| form.enabled.set(event_target_checked(&ev))
                    />
                    "Enabled"
                </label>

                // Transport-divergent fields.
                {move || match form.transport.get() {
                    McpTransport::Stdio => stdio_fields(form).into_any(),
                    McpTransport::Http => http_fields(view, form).into_any(),
                }}

                <Show when=move || form.error.get().is_some()>
                    <p class="mcp-error" role="alert">
                        {move || form.error.get().unwrap_or_default()}
                    </p>
                </Show>

                <div class="mcp-form-actions">
                    <button class="mcp-btn" on:click=move |_| form.close()>
                        "Cancel"
                    </button>
                    <button
                        class="mcp-btn primary"
                        disabled=move || view.mcp_busy.get()
                        on:click=on_save
                    >
                        "Save"
                    </button>
                </div>
            </div>
        }
    }

    /// The stdio (local process) config inputs.
    fn stdio_fields(form: FormState) -> impl IntoView {
        view! {
            {text_field("Command", form.command, "e.g. fileio-mcp")}
            {text_field("Arguments (space-separated)", form.args, "e.g. serve --root /data")}
            {text_field("Namespace (optional)", form.namespace, "e.g. files")}
            <div class="field">
                <label class="field-label">"Environment (KEY=value per line)"</label>
                <textarea
                    class="mcp-input mcp-textarea"
                    autocomplete="off"
                    placeholder="TOKEN=abc123"
                    prop:value=move || form.env.get()
                    on:input=move |ev| form.env.set(event_target_value(&ev))
                ></textarea>
                <Show when=move || form.editing.get()>
                    <p class="mcp-note muted">
                        "Environment variables aren't shown when editing — re-enter any that should be kept, or leave blank to clear them."
                    </p>
                </Show>
            </div>
        }
    }

    /// The http (remote endpoint) config inputs: url + an auth selector whose
    /// fields diverge by kind.
    fn http_fields(view: ViewSignals, form: FormState) -> impl IntoView {
        view! {
            {text_field("URL", form.url, "https://example.com/mcp/v1")}

            <div class="field">
                <span class="field-label">"Authentication"</span>
                <div class="segmented" role="group" aria-label="Authentication">
                    {[McpAuthKind::None, McpAuthKind::Bearer, McpAuthKind::OAuth]
                        .into_iter()
                        .map(|a| {
                            let active = move || form.auth.get() == a;
                            view! {
                                <button
                                    class="segment"
                                    class:active=active
                                    aria-pressed=move || if active() { "true" } else { "false" }
                                    on:click=move |_| form.auth.set(a)
                                >
                                    {auth_label(a)}
                                </button>
                            }
                        })
                        .collect_view()}
                </div>
            </div>

            {move || match form.auth.get() {
                McpAuthKind::None => ().into_any(),
                McpAuthKind::Bearer => {
                    view! {
                        <div class="field mcp-cred">
                            <label class="field-label">"Bearer token"</label>
                            <input
                                class="mcp-input"
                                type="password"
                                autocomplete="off"
                                placeholder="Paste token (stored write-only)"
                                prop:value=move || form.bearer_token.get()
                                on:input=move |ev| form.bearer_token.set(event_target_value(&ev))
                            />
                            <p class="mcp-note muted">
                                "Sent write-only to the daemon over your private tailnet; never shown here. Leave blank to keep the current token."
                            </p>
                        </div>
                    }
                        .into_any()
                }
                McpAuthKind::OAuth => oauth_fields(view, form).into_any(),
            }}
        }
    }

    /// The OAuth service-account picker + scopes. The account list comes from
    /// `ListServiceAccounts`; only the account *ref* is carried into the config —
    /// never a secret. Interactive sign-in itself is a desktop-host action (see
    /// the card's honest degradation note), so this form only wires the config.
    fn oauth_fields(view: ViewSignals, form: FormState) -> impl IntoView {
        view! {
            <div class="field">
                <label class="field-label">"Service account"</label>
                <select
                    class="select"
                    on:change=move |ev| form.oauth_account.set(event_target_value(&ev))
                >
                    {move || {
                        let selected = form.oauth_account.get();
                        // One homogeneous (value, label) list — placeholder first —
                        // mapped through a single closure so every <option> shares
                        // one concrete view type.
                        let mut opts = vec![(String::new(), "— choose account —".to_string())];
                        opts.extend(view.mcp_service_accounts.get().into_iter().map(|a| {
                            let label = if a.display_name.is_empty() {
                                a.id.clone()
                            } else {
                                a.display_name.clone()
                            };
                            (a.id, label)
                        }));
                        opts.into_iter()
                            .map(|(value, label)| {
                                let is_sel = value == selected;
                                view! {
                                    <option value=value selected=is_sel>
                                        {label}
                                    </option>
                                }
                            })
                            .collect_view()
                    }}
                </select>
                <Show when=move || view.mcp_service_accounts.get().is_empty()>
                    <p class="mcp-note muted">
                        "No service accounts configured. Add one from the desktop settings on the daemon host."
                    </p>
                </Show>
            </div>

            {text_field("Scopes (space or comma-separated)", form.scopes, "calendar.read calendar.write")}
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
                    class="mcp-input"
                    type="text"
                    autocomplete="off"
                    placeholder=placeholder
                    prop:value=move || value.get()
                    on:input=move |ev| value.set(event_target_value(&ev))
                />
            </div>
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stdio(name: &str) -> McpForm {
        McpForm {
            name: name.into(),
            command: "fileio-mcp".into(),
            ..McpForm::blank(McpTransport::Stdio)
        }
    }

    fn http(name: &str) -> McpForm {
        McpForm {
            name: name.into(),
            url: "https://x.example/mcp".into(),
            ..McpForm::blank(McpTransport::Http)
        }
    }

    // --- status_display -------------------------------------------------------

    #[test]
    fn status_display_covers_all_six_states() {
        assert_eq!(status_display("running"), ("mcp-dot ok", "Running"));
        assert_eq!(status_display("stopped"), ("mcp-dot neutral", "Stopped"));
        assert_eq!(status_display("disabled"), ("mcp-dot neutral", "Disabled"));
        assert_eq!(
            status_display("needs_auth"),
            ("mcp-dot warn", "Sign in required")
        );
        assert_eq!(
            status_display("auth_expired"),
            ("mcp-dot warn", "Sign in expired")
        );
        assert_eq!(status_display("error"), ("mcp-dot error", "Error"));
    }

    #[test]
    fn status_display_unknown_is_neutral() {
        assert_eq!(
            status_display("teleporting"),
            ("mcp-dot neutral", "Unknown")
        );
        assert_eq!(status_display(""), ("mcp-dot neutral", "Unknown"));
    }

    // --- transport_chip -------------------------------------------------------

    #[test]
    fn transport_chip_http_is_remote_else_local() {
        assert_eq!(transport_chip("http"), "remote");
        assert_eq!(transport_chip("stdio"), "local");
        assert_eq!(transport_chip("something-new"), "local");
    }

    // --- parse_env ------------------------------------------------------------

    #[test]
    fn parse_env_reads_key_value_lines_in_order() {
        assert_eq!(
            parse_env("TOKEN=abc\nDEBUG=1"),
            vec![
                ("TOKEN".to_string(), "abc".to_string()),
                ("DEBUG".to_string(), "1".to_string()),
            ]
        );
    }

    #[test]
    fn parse_env_skips_blank_and_malformed_lines() {
        // A line with no `=` and a line with a blank key are both dropped.
        assert_eq!(
            parse_env("\n  \nNOVALUE\n=novalue\nOK=1\n"),
            vec![("OK".to_string(), "1".to_string())]
        );
    }

    #[test]
    fn parse_env_value_may_contain_equals() {
        assert_eq!(
            parse_env("QUERY=a=b=c"),
            vec![("QUERY".to_string(), "a=b=c".to_string())]
        );
    }

    #[test]
    fn parse_env_trims_key_and_value() {
        assert_eq!(
            parse_env("  KEY = val \n"),
            vec![("KEY".to_string(), "val".to_string())]
        );
    }

    // --- split_args / split_scopes -------------------------------------------

    #[test]
    fn split_args_splits_on_whitespace_runs() {
        assert_eq!(
            split_args("serve   --root  /data"),
            vec!["serve", "--root", "/data"]
        );
    }

    #[test]
    fn split_args_empty_is_empty() {
        assert!(split_args("   ").is_empty());
        assert!(split_args("").is_empty());
    }

    #[test]
    fn split_scopes_splits_on_whitespace_and_commas() {
        assert_eq!(split_scopes("a b,c ,  d"), vec!["a", "b", "c", "d"]);
        assert!(split_scopes("").is_empty());
    }

    // --- bearer_secret_ref ----------------------------------------------------

    #[test]
    fn bearer_secret_ref_appends_token_suffix() {
        assert_eq!(bearer_secret_ref("gmail"), "gmail_token");
    }

    // --- mcp_secret_command (wire shape + redaction) --------------------------

    #[test]
    fn mcp_secret_command_wire_shape() {
        let cmd = mcp_secret_command("gmail_token".into(), "ya29.tok".into());
        let json = serde_json::to_string(&cmd).expect("serializes");
        assert_eq!(
            json,
            r#"{"set_mcp_secret":{"id":"gmail_token","value":"ya29.tok"}}"#
        );
    }

    #[test]
    fn mcp_secret_command_redacts_value_in_debug() {
        let cmd = mcp_secret_command("gmail_token".into(), "ya29.supersecret".into());
        let dump = format!("{cmd:?}");
        assert!(!dump.contains("ya29.supersecret"), "token leaked: {dump}");
    }

    // --- build: stdio ---------------------------------------------------------

    #[test]
    fn build_stdio_emits_exact_config_json() {
        let form = McpForm {
            args: "serve --root /data".into(),
            namespace: "files".into(),
            env: "TOKEN=abc\nDEBUG=1".into(),
            ..stdio("files")
        };
        let built = form.build().expect("builds");
        assert!(!built.editing);
        assert_eq!(built.name, "files");
        assert_eq!(built.secret, None);
        // env is a BTreeMap in the DTO → keys sorted (DEBUG before TOKEN),
        // deterministic on the wire.
        assert_eq!(
            built.config_json,
            r#"{"name":"files","enabled":true,"command":"fileio-mcp","args":["serve","--root","/data"],"namespace":"files","env":{"DEBUG":"1","TOKEN":"abc"}}"#
        );
    }

    #[test]
    fn build_stdio_omits_empty_optionals() {
        let built = stdio("bare").build().expect("builds");
        assert_eq!(
            built.config_json,
            r#"{"name":"bare","enabled":true,"command":"fileio-mcp"}"#
        );
    }

    #[test]
    fn build_carries_disabled_flag() {
        let form = McpForm {
            enabled: false,
            ..stdio("x")
        };
        let built = form.build().expect("builds");
        assert!(built.config_json.contains(r#""enabled":false"#));
    }

    // --- build: http bearer ---------------------------------------------------

    #[test]
    fn build_http_bearer_emits_config_and_secret() {
        let form = McpForm {
            url: "https://gmailmcp.googleapis.com/mcp/v1".into(),
            auth: McpAuthKind::Bearer,
            bearer_token: "  ya29.token \n".into(),
            ..http("gmail")
        };
        let built = form.build().expect("builds");
        assert_eq!(
            built.config_json,
            r#"{"name":"gmail","enabled":true,"http":{"url":"https://gmailmcp.googleapis.com/mcp/v1","auth_bearer_secret":"gmail_token"}}"#
        );
        // The token is trimmed and written under the `{name}_token` ref.
        assert_eq!(
            built.secret,
            Some(("gmail_token".to_string(), "ya29.token".to_string()))
        );
    }

    #[test]
    fn build_http_bearer_blank_token_writes_no_secret() {
        // Write-only: a blank token field never wipes a stored token — but the
        // config still references the ref so the server is honestly "bearer,
        // token pending" rather than silently switching to unauthenticated.
        let form = McpForm {
            auth: McpAuthKind::Bearer,
            bearer_token: "   ".into(),
            ..http("gmail")
        };
        let built = form.build().expect("builds");
        assert_eq!(built.secret, None);
        assert!(
            built
                .config_json
                .contains(r#""auth_bearer_secret":"gmail_token""#)
        );
    }

    // --- build: http oauth ----------------------------------------------------

    #[test]
    fn build_http_oauth_emits_account_ref_and_scopes() {
        let form = McpForm {
            url: "https://cal.example/mcp".into(),
            auth: McpAuthKind::OAuth,
            oauth_account: "work-google".into(),
            scopes: "calendar.read, calendar.write".into(),
            ..http("cal")
        };
        let built = form.build().expect("builds");
        // OAuth carries only the account ref + scopes — never a secret value.
        assert_eq!(built.secret, None);
        assert_eq!(
            built.config_json,
            r#"{"name":"cal","enabled":true,"http":{"url":"https://cal.example/mcp","oauth_account":"work-google","scopes":["calendar.read","calendar.write"]}}"#
        );
    }

    // --- build: validation ----------------------------------------------------

    #[test]
    fn build_requires_command_for_stdio() {
        let form = McpForm {
            command: "   ".into(),
            ..stdio("x")
        };
        assert!(form.build().is_err());
    }

    #[test]
    fn build_requires_url_for_http() {
        let form = McpForm {
            url: "".into(),
            ..http("x")
        };
        assert!(form.build().is_err());
    }

    #[test]
    fn build_requires_account_for_oauth() {
        let form = McpForm {
            auth: McpAuthKind::OAuth,
            oauth_account: "  ".into(),
            ..http("x")
        };
        assert!(form.build().is_err());
    }

    #[test]
    fn build_requires_valid_name_on_create() {
        assert!(stdio("").build().is_err());
        assert!(stdio("has space").build().is_err());
        assert!(stdio("ok-name_1").build().is_ok());
    }

    #[test]
    fn build_edit_does_not_revalidate_locked_name() {
        // On edit the name is the already-stored (locked) one, so build trusts
        // it rather than re-running the create-time slug check.
        let form = McpForm {
            editing: true,
            name: "already.there".into(),
            ..stdio("already.there")
        };
        let built = form.build().expect("builds");
        assert!(built.editing);
        assert_eq!(built.name, "already.there");
    }

    // --- from_view (edit prefill) --------------------------------------------

    #[test]
    fn from_view_prefills_stdio_editor() {
        let view = McpServerView {
            name: "files".into(),
            command: "fileio-mcp".into(),
            args: vec!["serve".into(), "--root".into(), "/data".into()],
            namespace: Some("files".into()),
            enabled: true,
            status: "running".into(),
            transport: "stdio".into(),
            target: "fileio-mcp".into(),
            ..Default::default()
        };
        let f = McpForm::from_view(&view);
        assert!(f.editing);
        assert_eq!(f.transport, McpTransport::Stdio);
        assert_eq!(f.name, "files");
        assert_eq!(f.command, "fileio-mcp");
        assert_eq!(f.args, "serve --root /data");
        assert_eq!(f.namespace, "files");
        // The view carries no env — never pre-filled.
        assert_eq!(f.env, "");
    }

    #[test]
    fn from_view_prefills_http_bearer_editor() {
        let view = McpServerView {
            name: "gh".into(),
            enabled: true,
            status: "running".into(),
            transport: "http".into(),
            target: "https://gh.example/mcp".into(),
            auth_kind: Some("bearer".into()),
            ..Default::default()
        };
        let f = McpForm::from_view(&view);
        assert_eq!(f.transport, McpTransport::Http);
        assert_eq!(f.auth, McpAuthKind::Bearer);
        assert_eq!(f.url, "https://gh.example/mcp");
        // Write-only: the token is never echoed / pre-filled.
        assert_eq!(f.bearer_token, "");
    }

    #[test]
    fn from_view_prefills_http_oauth_editor() {
        let view = McpServerView {
            name: "cal".into(),
            enabled: true,
            status: "needs_auth".into(),
            transport: "http".into(),
            target: "https://cal.example/mcp".into(),
            auth_kind: Some("oauth".into()),
            oauth_account_ref: Some("work-google".into()),
            oauth_scopes: vec!["calendar.read".into()],
            oauth_authorized: Some(false),
            ..Default::default()
        };
        let f = McpForm::from_view(&view);
        assert_eq!(f.transport, McpTransport::Http);
        assert_eq!(f.auth, McpAuthKind::OAuth);
        assert_eq!(f.url, "https://cal.example/mcp");
        assert_eq!(f.oauth_account, "work-google");
        assert_eq!(f.scopes, "calendar.read");
    }
}
