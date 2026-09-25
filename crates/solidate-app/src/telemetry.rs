//! Process-wide `tracing` subscriber for the Solidate binaries.
//!
//! Output goes to stderr (stdout carries the MCP stdio protocol and CLI results).
//! `RUST_LOG` sets the filter (default `info`, or the binary's own default);
//! `SOLIDATE_LOG_FORMAT=json` switches to one JSON object per line.

use tracing_subscriber::EnvFilter;

/// Installs the subscriber. `default_filter` applies when `RUST_LOG` is unset.
/// Does nothing if a subscriber is already installed.
pub fn init(default_filter: &str) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr);
    let _ = if std::env::var("SOLIDATE_LOG_FORMAT").is_ok_and(|v| v == "json") {
        builder.json().flatten_event(true).try_init()
    } else {
        builder.try_init()
    };
}
