use tracing_appender::{non_blocking::WorkerGuard, rolling};
use tracing_subscriber::{layer::SubscriberExt, EnvFilter, Layer};

pub fn start_logs(path: String) -> WorkerGuard {
    let appender = rolling::never(path, "logs.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(appender);

    let terminal_filter = EnvFilter::new("quark=trace");
    let file_filter = EnvFilter::new("quark=info");

    let terminal_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stdout)
        .with_filter(terminal_filter);
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_filter(file_filter);

    let subscriber = tracing_subscriber::registry()
        .with(terminal_layer)
        .with(file_layer);

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    guard
}
