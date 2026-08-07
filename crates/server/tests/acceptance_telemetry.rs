//! Telemetry acceptance criteria that need no daemon, no subprocess and no access to
//! `crates/server`'s own (private) modules - so these live as ordinary integration
//! tests rather than in-module unit tests.
//!
//! `default_build_pulls_no_opentelemetry` and `wasm_workspace_is_untouched` pass
//! already, before `main.rs` is wired to `adelie_telemetry::init`: the first is a
//! property of the dependency as declared in `Cargo.toml` (this crate's own `otel`
//! feature is off by default), and the second guards a boundary this ticket must not
//! move. Both stay green throughout as regression guards.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use adelie_telemetry::Config;

/// Acceptance (epic D5): a second `init` call in one process is a no-op, not a panic.
/// `init` installs a process-global subscriber, so every test in this binary that calls
/// it shares one process - which is exactly what this test needs to prove.
#[test]
fn telemetry_init_is_idempotent() {
    let first = adelie_telemetry::init(
        Config::new("adele-web-ui-acceptance").with_metrics_dump_interval(Duration::ZERO),
    );
    assert!(first.is_ok(), "the first init must install telemetry");

    let second = adelie_telemetry::init(
        Config::new("adele-web-ui-acceptance-again").with_metrics_dump_interval(Duration::ZERO),
    );
    assert!(
        second.is_ok(),
        "a second init in the same process must be a no-op that returns, not a panic"
    );

    drop(second);
    drop(first);

    // The live subscriber must still work after the inert second guard drops.
    tracing::info!("adele-web-ui telemetry still installed after the inert guard dropped");
}

/// Acceptance: a default-feature build of the BFF resolves no opentelemetry crate.
/// `adelie-telemetry`'s `otel` feature is the only thing that pulls one in, and this
/// crate's own `otel` feature (`Cargo.toml`) only exists to pass that switch through -
/// off by default, so a default `cargo tree` for this package must show nothing
/// OTLP-shaped.
#[test]
fn default_build_pulls_no_opentelemetry() {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let manifest_dir = env!("CARGO_MANIFEST_DIR");

    let output = Command::new(&cargo)
        .args(["tree", "--edges", "normal", "--prefix", "none"])
        .current_dir(manifest_dir)
        .output()
        .expect("cargo tree must run");
    assert!(
        output.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let tree = String::from_utf8_lossy(&output.stdout);
    let hits: Vec<&str> = tree
        .lines()
        .filter(|line| line.to_ascii_lowercase().starts_with("opentelemetry"))
        .collect();

    assert!(
        hits.is_empty(),
        "a default-feature build resolves opentelemetry crates: {hits:?}"
    );
}

/// Acceptance: `crates/web` (the Leptos wasm SPA, its own workspace) gains no
/// dependency from this ticket, and the native gate still never builds it.
/// `adelie-telemetry` needs tokio for OTLP export, so the browser target stays out of
/// scope for the whole epic (mcp-core#38's "Out of scope" list).
#[test]
fn wasm_workspace_is_untouched() {
    let root_manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
    let root = std::fs::read_to_string(&root_manifest).expect("the root Cargo.toml reads");
    assert!(
        root.contains(r#"exclude = ["crates/web"]"#),
        "the root workspace must keep excluding crates/web from the native gate"
    );

    let web_manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../web/Cargo.toml");
    let web = std::fs::read_to_string(&web_manifest).expect("crates/web/Cargo.toml reads");
    for marker in ["adelie-telemetry", "opentelemetry", "tracing-opentelemetry"] {
        assert!(
            !web.contains(marker),
            "crates/web must gain no telemetry dependency; found {marker:?} in its Cargo.toml"
        );
    }
}

/// Acceptance, the other half of the boundary: the native gate - `cargo metadata` at
/// the workspace root - must not resolve the wasm SPA package or anything only it
/// depends on (leptos, gloo).
#[test]
fn native_gate_does_not_resolve_the_wasm_spa() {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");

    let output = Command::new(&cargo)
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(&workspace_root)
        .output()
        .expect("cargo metadata must run");
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let metadata = String::from_utf8_lossy(&output.stdout);
    assert!(
        !metadata.contains("adele-web-ui-web"),
        "the native workspace metadata must not include the wasm SPA package"
    );
}
