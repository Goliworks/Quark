use tracing_appender::{non_blocking::WorkerGuard, rolling};
use tracing_subscriber::layer::SubscriberExt;

pub fn start_logs(path: String) -> WorkerGuard {
    let appender = rolling::never(path, "quark.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(appender);

    let terminal_layer = tracing_subscriber::fmt::layer().with_writer(std::io::stdout);
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_ansi(false);

    let subscriber = tracing_subscriber::registry()
        .with(terminal_layer)
        .with(file_layer);

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    guard
}
