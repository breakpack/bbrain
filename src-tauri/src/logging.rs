use std::path::Path;

use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// File logging for the app data `logs/` directory. Authorization headers,
/// request bodies and provider response bodies are never passed to `tracing`,
/// so no redaction layer is needed here — see DEVELOPMENT.md §16.1.
pub fn init(logs_dir: &Path) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let appender = tracing_appender::rolling::daily(logs_dir, "bbrain.log");
    let (writer, guard) = tracing_appender::non_blocking(appender);

    let filter = EnvFilter::try_from_env("BBRAIN_LOG")
        .unwrap_or_else(|_| EnvFilter::new(if cfg!(debug_assertions) { "debug" } else { "info" }));

    let file_layer = fmt::layer().with_ansi(false).with_writer(writer);
    let stdout_layer = fmt::layer().with_writer(std::io::stdout);

    tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(stdout_layer)
        .try_init()
        .ok()?;

    Some(guard)
}
