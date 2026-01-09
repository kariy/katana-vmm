use anyhow::{Context, Result};
use axum::{
    extract::Extension,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use serde_json::json;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::UnixListener;
use tower::{util::ServiceExt, ServiceBuilder};
use tower_http::trace::TraceLayer;
use tracing::{info, Level};

mod api;
mod error;
mod models;
mod state;

use state::DaemonState;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt().with_max_level(Level::INFO).init();

    info!("Starting Katana Hypervisor Daemon");

    // Socket path
    let socket_path = PathBuf::from("/var/run/katana/daemon.sock");

    // Remove old socket if exists
    if socket_path.exists() {
        fs::remove_file(&socket_path).context("Failed to remove existing socket file")?;
    }

    // Create socket directory if needed
    if let Some(parent) = socket_path.parent() {
        fs::create_dir_all(parent).context("Failed to create socket directory")?;
    }

    // Initialize daemon state
    let state = Arc::new(DaemonState::new()?);

    info!("Initializing daemon state");

    // Bind UNIX socket
    let listener = UnixListener::bind(&socket_path).context("Failed to bind UNIX socket")?;

    // Set permissions (0660 for group access)
    fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o660))
        .context("Failed to set socket permissions")?;

    info!("Listening on UNIX socket: {}", socket_path.display());

    // Build router
    let app = build_router(state);

    // Accept connections
    loop {
        let (stream, _addr) = listener
            .accept()
            .await
            .context("Failed to accept connection")?;

        let io = TokioIo::new(stream);
        let tower_service = app.clone();

        tokio::spawn(async move {
            let hyper_service =
                hyper::service::service_fn(move |request: hyper::Request<Incoming>| {
                    tower_service.clone().oneshot(request)
                });

            if let Err(err) = http1::Builder::new()
                .serve_connection(io, hyper_service)
                .await
            {
                tracing::error!("Error serving connection: {:?}", err);
            }
        });
    }
}

fn build_router(state: Arc<DaemonState>) -> Router {
    let layer = ServiceBuilder::new()
        .layer(TraceLayer::new_for_http())
        .layer(Extension(state));

    Router::new()
        .route("/health", get(health_check))
        .route("/version", get(get_version))
        .nest("/api/v1", api_routes())
        .layer(layer)
}

fn api_routes() -> Router {
    Router::new()
        // Instance CRUD
        .route(
            "/instances",
            get(api::list_instances).post(api::create_instance),
        )
        .route(
            "/instances/:name",
            get(api::get_instance).delete(api::delete_instance),
        )
        // Instance operations
        .route("/instances/:name/start", post(api::start_instance))
        .route("/instances/:name/stop", post(api::stop_instance))
        .route("/instances/:name/pause", post(api::pause_instance))
        .route("/instances/:name/resume", post(api::resume_instance))
        .route("/instances/:name/suspend", post(api::suspend_instance))
        .route("/instances/:name/reset", post(api::reset_instance))
        // Monitoring
        .route("/instances/:name/logs", get(api::get_logs))
        .route("/instances/:name/logs/stream", get(api::stream_logs))
        .route("/instances/:name/stats", get(api::get_stats))
}

// Health check endpoint
async fn health_check() -> impl IntoResponse {
    Json(json!({
        "status": "ok",
        "service": "katana-daemon"
    }))
}

// Version endpoint
async fn get_version() -> impl IntoResponse {
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "name": env!("CARGO_PKG_NAME")
    }))
}
