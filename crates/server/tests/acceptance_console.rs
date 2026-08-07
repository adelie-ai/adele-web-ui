//! Console acceptance criteria.
//!
//! Runs a separate process on purpose: the only honest way to prove nothing reaches
//! stdout is to look at the real file descriptor of a real process, not a layer
//! configured with a capture writer.
//!
//! This passes as soon as `examples/log_probe.rs` exists, because the property under
//! test - stderr, never stdout - is guaranteed by `adelie-telemetry` itself (epic D1).
//! There is nothing in `crates/server`'s own code for this one to catch; it is a
//! regression guard against a future `main.rs` that routes a writer at stdout.

use std::path::PathBuf;
use std::process::Command;

/// Where `cargo test` leaves the example binaries: one directory across from the test
/// binary itself, which sits in `target/<profile>/deps/`. `cargo test` builds examples,
/// so it is always there by the time this runs.
fn probe_binary() -> PathBuf {
    let mut path = std::env::current_exe().expect("a test binary knows its own path");
    path.pop(); // the test binary's file name
    if path.ends_with("deps") {
        path.pop();
    }
    path.push("examples");
    path.push("log_probe");
    path
}

/// Acceptance (epic D1): nothing the BFF logs reaches stdout, at any level.
#[test]
fn logs_go_to_stderr_not_stdout() {
    let probe = probe_binary();
    assert!(
        probe.is_file(),
        "the log probe example must be built before this test can prove anything; \
         expected it at {}",
        probe.display()
    );

    let output = Command::new(&probe)
        .env("RUST_LOG", "trace")
        .output()
        .expect("the probe must run");

    assert!(
        output.status.success(),
        "the probe must exit cleanly, otherwise an empty stdout proves nothing"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        stdout.trim(),
        "STDOUT-MARKER",
        "only the probe's own marker may reach stdout. stdout was: {stdout:?}"
    );

    for level in ["INFO", "WARN"] {
        assert!(
            stderr.contains(level),
            "{level} must reach stderr, or the console layer is not doing its job. \
             stderr was: {stderr:?}"
        );
    }
}
