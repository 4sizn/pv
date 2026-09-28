//! Composition root: owns listener, cleanup task, shutdown and concrete dependencies.
use std::time::Instant;

use parentview_signaling::{config::Config, router, AppState};
use tokio::sync::watch;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let config = Config::from_env().map_err(std::io::Error::other)?;
    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    tracing::info!(address = %listener.local_addr()?, "signaling listening");
    let state = AppState::new(config).map_err(std::io::Error::other)?;
    let (stop_cleanup, mut cleanup_stopped) = watch::channel(false);
    let cleanup_state = state.clone();
    let cleanup = tokio::spawn(async move {
        let mut interval = tokio::time::interval(cleanup_state.config.cleanup_interval);
        loop {
            tokio::select! {
                _ = interval.tick() => cleanup_state.cleanup(Instant::now()),
                _ = cleanup_stopped.changed() => break,
            }
        }
    });
    let shutdown_state = state.clone();
    let result = axum::serve(listener, router(state.clone()))
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            shutdown_state.disconnect_all();
        })
        .await;
    state.disconnect_all();
    stop_cleanup.send_replace(true);
    cleanup.await?;
    result?;
    Ok(())
}
