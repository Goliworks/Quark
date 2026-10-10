use tracing_appender::{
    non_blocking::WorkerGuard,
    rolling::{RollingFileAppender, Rotation},
};
use tracing_subscriber::{layer::SubscriberExt, EnvFilter, Layer};

use crate::config::{self, RollingStrategy};

pub fn start_logs(conf: &config::Log) -> WorkerGuard {
    let appender = build_appender(&conf.path, conf);
    let (non_blocking, guard) = tracing_appender::non_blocking::NonBlockingBuilder::default()
        .buffered_lines_limit(conf.buffer_lines_limit as usize)
        .lossy(conf.lossy)
        .finish(appender);

    #[cfg(debug_assertions)]
    let terminal_filter = EnvFilter::new("quark=trace");

    let file_filter = EnvFilter::new("quark=info");

    #[cfg(debug_assertions)]
    let terminal_layer = tracing_subscriber::fmt::layer()
        .with_file(false)
        .with_writer(std::io::stdout)
        .with_filter(terminal_filter);

    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_file(false)
        .with_line_number(false)
        .with_filter(file_filter);

    #[cfg(debug_assertions)]
    let subscriber = tracing_subscriber::registry()
        .with(terminal_layer)
        .with(file_layer);

    #[cfg(not(debug_assertions))]
    let subscriber = tracing_subscriber::registry().with(file_layer);

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    guard
}

fn build_appender(path: &str, config: &config::Log) -> RollingFileAppender {
    RollingFileAppender::builder()
        .rotation(match config.rotation {
            RollingStrategy::Never => Rotation::NEVER,
            RollingStrategy::Minutely => Rotation::MINUTELY,
            RollingStrategy::Hourly => Rotation::HOURLY,
            RollingStrategy::Daily => Rotation::DAILY,
            RollingStrategy::Weekly => Rotation::WEEKLY,
        })
        .filename_prefix(&config.prefix)
        .filename_suffix(&config.suffix)
        .build(path)
        .expect("failed to initialize log appender")
}
