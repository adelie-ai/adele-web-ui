//! Confirms the BFF's telemetry writes logs to stderr, never stdout.
//!
//! Run by the acceptance test `logs_go_to_stderr_not_stdout` (`tests/acceptance_console.rs`)
//! in a fresh process: the only honest way to prove nothing reaches stdout is to look at
//! the real file descriptor of a process that installed telemetry the way the BFF binary
//! does. The MCP stdio transport frames JSON-RPC on stdout elsewhere in the fleet, so a
//! stray log line there is the failure this exists to catch (epic D1).
//!
//! The one line this writes to stdout is the marker the test looks for, so an empty
//! stdout caused by the probe failing to start cannot be mistaken for a pass.

use std::time::Duration;

use adelie_telemetry::Config;

fn main() {
    println!("STDOUT-MARKER");

    let guard = adelie_telemetry::init(
        Config::new("adele-web-ui")
            .with_default_filter("info")
            .with_metrics_dump_interval(Duration::ZERO),
    )
    .expect("telemetry must install");

    tracing::info!("adele-web-ui log probe: info line");
    tracing::warn!("adele-web-ui log probe: warn line");

    drop(guard);
}
