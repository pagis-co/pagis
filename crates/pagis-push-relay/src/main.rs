//! The Push Relay as one command. It reads its settings from the
//! `PUSH_RELAY_*` variables, opens its SQLite file and serves until
//! SIGINT or SIGTERM.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context;
use pagis_push_relay::{
    ApnsBaseUrls, ApnsTransport, Clock, FCM_BASE_URL, FcmTransport, ServiceAccount, Settings,
    SystemClock, Transports,
};
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
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let mut transports = Transports::default();
    match &settings.apns {
        Some(apns) => {
            let apns = ApnsTransport::new(apns, ApnsBaseUrls::apple(), clock.clone())?;
            transports = transports.with_ios(Arc::new(apns));
        }
        None => tracing::info!(
            "no PUSH_RELAY_APNS_* variable is set, so the relay serves no ios registration"
        ),
    }
    match &settings.fcm {
        Some(fcm) => {
            let tokens = ServiceAccount::read(&fcm.credentials_path)?;
            let fcm = FcmTransport::new(&fcm.project_id, Arc::new(tokens), FCM_BASE_URL)?;
            transports = transports.with_android(Arc::new(fcm));
        }
        None => tracing::info!(
            "no PUSH_RELAY_FCM_* variable is set, so the relay serves no android registration"
        ),
    }
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

    let router = pagis_push_relay::router(
        pool,
        settings.public_origin,
        settings.trusted_proxy,
        transports,
        clock,
    );
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
