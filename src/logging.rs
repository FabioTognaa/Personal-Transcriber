use tracing_subscriber::EnvFilter;

pub fn init(verbosity: u8) {
    let default_level = match verbosity {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };

    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .try_init()
        .ok();

    // Route whisper.cpp/ggml logging through `tracing` so its verbosity follows
    // `-v` instead of always writing to stderr. Global and idempotent.
    whisper_rs::install_logging_hooks();
}
