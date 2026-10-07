//! The Push Relay as one command. It reads its settings from the
//! `PUSH_RELAY_*` variables, opens its SQLite file and serves until
//! SIGINT or SIGTERM.

use std::net::SocketAddr;

use anyhow::Context;
use pagis_push_relay::Settings;
use tokio::signal::unix::{SignalKind, signal};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_ansi(false)
        .init();

    let settings = Settings::from_var(&|name| std::env::var(name).ok())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    let pool = pagis_push_relay::connect(&settings.database)
        .await
        .with_context(|| {
            format!(
                "cannot open the SQLite file {} that PUSH_RELAY_DATABASE names",
                settings.database.display()
            )
        })?;
    let listener = tokio::net::TcpListener::bind(settings.bind)
        .await
        .with_context(|| {
            format!(
                "cannot listen on {} that PUSH_RELAY_BIND names",
                settings.bind
            )
        })?;
    let address = listener.local_addr()?;
    tracing::info!("the Push Relay listens on http://{address}");

    let router = pagis_push_relay::router(pool, settings.public_origin, settings.trusted_proxy);
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        tokio::select! {
            _ = interrupt.recv() => {}
            _ = terminate.recv() => {}
        }
    })
    .await?;
    Ok(())
}
