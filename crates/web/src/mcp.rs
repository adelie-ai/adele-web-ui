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

use desktop_assistant_api_model::{Command, McpServerView};

// ===========================================================================
// Pure logic (host-testable)
// ===========================================================================

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
pub fn status_display(_status: &str) -> (&'static str, &'static str) {
    todo!("issue #55")
}

/// The transport chip label: an HTTP server is `"remote"`, anything else
/// (stdio) is `"local"`.
pub fn transport_chip(_transport: &str) -> &'static str {
    todo!("issue #55")
}

/// Parse an env textarea into ordered `(KEY, value)` pairs. Each non-blank line
/// is `KEY=value`; the key is trimmed and the value is everything after the
/// first `=` (values may themselves contain `=`), also trimmed for paste
/// hygiene. Lines without a `=`, or with a blank key, are skipped — a malformed
/// line is dropped, never turned into a half-entry.
pub fn parse_env(_text: &str) -> Vec<(String, String)> {
    todo!("issue #55")
}

/// Split a space-separated args string into argv tokens. Any run of whitespace
/// separates; empty tokens are dropped. Deliberately simple — a server needing
/// shell-quoted args with embedded spaces is a rare case the v1 form leaves to a
/// direct config edit.
pub fn split_args(_text: &str) -> Vec<String> {
    todo!("issue #55")
}

/// Split an OAuth scopes string on whitespace and/or commas into individual
/// scopes, dropping empties.
pub fn split_scopes(_text: &str) -> Vec<String> {
    todo!("issue #55")
}

/// The secrets.toml ref a server's bearer token is stored under. Convention:
/// `{name}_token`, so a server's config can reference its token by a stable id
/// the user never has to hand-edit.
pub fn bearer_secret_ref(_name: &str) -> String {
    todo!("issue #55")
}

/// Build the [`Command::SetMcpSecret`] that stores a bearer token value under
/// `id`. The value is wrapped in [`desktop_assistant_api_model::Secret`] so it
/// can't leak via `Debug`.
pub fn mcp_secret_command(_id: String, _value: String) -> Command {
    todo!("issue #55")
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
    pub fn from_view(_view: &McpServerView) -> Self {
        todo!("issue #55")
    }

    /// Validate + assemble the form into the command inputs: the target name
    /// (typed + validated on create, immutable on edit), the `config_json`
    /// string [`Command::UpsertMcpServer`] receives, and the optional bearer
    /// secret `(ref, value)` to write *first*. `Err` carries a human-readable
    /// reason.
    pub fn build(&self) -> Result<BuiltMcpServer, String> {
        todo!("issue #55")
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
