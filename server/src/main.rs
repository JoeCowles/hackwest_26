use anyhow::{Context, Result};
use clap::Parser;
use orchard_server::{api, config::Config, read_api, store::AppState, workers};
use std::{
    net::TcpListener,
    sync::{Arc, RwLock},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[cfg(feature = "desktop")]
mod desktop;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "orchard_server=info".into()),
        )
        .init();
    let config = Config::parse();
    let files = config.prepare()?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let state = runtime.block_on(AppState::open(
        &files.directory.join("orchard.sqlite3"),
        &files.admin_token,
    ))?;
    let listener = TcpListener::bind(config.bind).context("Unable to bind server address")?;
    listener.set_nonblocking(true)?;
    let address = listener.local_addr()?;
    let url = format!(
        "{}://{}",
        if config.tls_cert.is_some() {
            "https"
        } else {
            "http"
        },
        address
    );
    let stop = CancellationToken::new();
    let server_status = Arc::new(RwLock::new(format!("Listening on {url}")));
    read_api::write_viewer_file(&files.directory, &state.viewer_token)?;
    // The workspace includes both rustls providers; select the server provider explicitly.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let router = api::router(state.clone())
        .merge(read_api::router(state.clone()))
        .merge(orchard_server::cider_api::router(state.clone()));
    let status = server_status.clone();
    let shutdown = stop.clone();
    let tls_paths = config.tls_cert.clone().zip(config.tls_key.clone());
    let server = runtime.spawn(async move {
        let result: Result<()> = async {
            if let Some((cert, key)) = tls_paths {
                let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key).await?;
                let handle = axum_server::Handle::new();
                let graceful = handle.clone();
                tokio::spawn(async move {
                    shutdown.cancelled().await;
                    graceful.graceful_shutdown(Some(Duration::from_secs(5)));
                });
                axum_server::from_tcp_rustls(listener, tls)?
                    .handle(handle)
                    .serve(router.into_make_service())
                    .await?;
            } else {
                axum::serve(tokio::net::TcpListener::from_std(listener)?, router)
                    .with_graceful_shutdown(shutdown.cancelled_owned())
                    .await?;
            }
            Ok(())
        }
        .await;
        if let Ok(mut status) = status.write() {
            *status = match &result {
                Ok(()) => "Server stopped".into(),
                Err(e) => format!("Server failed: {e}"),
            };
        }
        result
    });
    let worker = runtime.spawn(workers::run(state.clone(), stop.clone()));
    let notifications = runtime.spawn(orchard_server::notifications::run(state.clone(), stop.clone()));
    tracing::info!(address=%url,credentials=%files.credential_path.display(),"Orchard Server started; credentials are not logged");
    let desktop_result: Result<()>;
    #[cfg(feature = "desktop")]
    {
        if config.headless {
            runtime.block_on(async {tokio::select!{_=tokio::signal::ctrl_c()=>{},_=wait_for_failure(server_status.clone())=>{}}});
            desktop_result = Ok(());
        } else {
            desktop_result = desktop::run(
                state.clone(),
                runtime.handle().clone(),
                server_status,
                url,
                files.admin_token.clone(),
                files.directory.clone(),
            );
        }
    }
    #[cfg(not(feature = "desktop"))]
    {
        runtime.block_on(async {tokio::select!{_=tokio::signal::ctrl_c()=>{},_=wait_for_failure(server_status.clone())=>{}}});
        desktop_result = Ok(());
    }
    stop.cancel();
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(8), server)
            .await
            .context("Server shutdown timed out")???;
        worker.await?;
        notifications.await?;
        state.db.close().await;
        Ok::<(), anyhow::Error>(())
    })?;
    desktop_result
}

async fn wait_for_failure(status: Arc<RwLock<String>>) {
    loop {
        if status.read().is_ok_and(|s| s.starts_with("Server failed:")) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}
