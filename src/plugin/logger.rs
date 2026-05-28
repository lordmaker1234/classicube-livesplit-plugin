use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::plugin::module::Module;

pub struct LoggerModule;

impl LoggerModule {
    pub fn init() -> Self {
        let debug = cfg!(debug_assertions);
        let level = if debug { "debug" } else { "info" };
        let my_crate_name = env!("CARGO_PKG_NAME").replace('-', "_");

        let filter = EnvFilter::builder()
            .with_default_directive(format!("{my_crate_name}={level}").parse().unwrap())
            .from_env_lossy();

        if let Err(e) = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(false)
            .with_thread_ids(false)
            .with_thread_names(false)
            .with_ansi(true)
            .without_time()
            .try_init()
        {
            eprintln!("failed to init tracing subscriber: {e}");
        }

        info!(
            "{} v{} init",
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION")
        );

        Self
    }
}

impl Module for LoggerModule {}
