//! The thin binary: parses configuration, connects to the database and
//! listens. Every decision beyond that — the router, the transport rules,
//! the routes themselves — lives in the library so a test can build the
//! identical thing in-process (`lib.rs`).
//!
//! Refuses to start, with a message on stderr and a non-zero exit, on a
//! missing or malformed configuration value, on an unsafe configuration
//! combination, or on a database whose schema is behind this build (issue
//! #45's own acceptance criteria). None of those are things to keep running
//! and hope about.

use std::process::ExitCode;

use payroll_app::SaltDatabase;
use salt_server::{AppState, ServerConfig, build_router};

#[tokio::main]
async fn main() -> ExitCode {
    // Falls back to `info` rather than to `EnvFilter`'s own default, which
    // is to log nothing at all: with `RUST_LOG` unset — the normal case for
    // a container — every request line, and every internal-error line
    // carrying a request id, would be silently discarded. "A per-request id
    // appears in every log line" (issue #45) is worth nothing if there are
    // no log lines.
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let config = match ServerConfig::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("salt-server: refusing to start: {error}");
            return ExitCode::FAILURE;
        }
    };

    let db = match SaltDatabase::connect(&config.database).await {
        Ok(db) => db,
        Err(error) => {
            eprintln!("salt-server: refusing to start: {error}");
            return ExitCode::FAILURE;
        }
    };

    let listener = match tokio::net::TcpListener::bind(config.bind_addr).await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("salt-server: could not bind {}: {error}", config.bind_addr);
            return ExitCode::FAILURE;
        }
    };

    tracing::info!(bind_addr = %config.bind_addr, environment = ?config.environment, "salt-server listening");

    let router = build_router(AppState::new(db, !config.insecure_cookies));
    let served = axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await;
    if let Err(error) = served {
        eprintln!("salt-server: server error: {error}");
        return ExitCode::FAILURE;
    }

    tracing::info!("salt-server stopped");
    ExitCode::SUCCESS
}

/// Resolves on the first `SIGINT` or `SIGTERM`, so a deploy or a `docker
/// stop` lets in-flight requests finish instead of severing them mid-write.
/// `SIGTERM` is the one an orchestrator actually sends; `SIGINT` is the one
/// a person pressing Ctrl-C sends, and both mean the same thing here.
async fn shutdown_signal() {
    let interrupt = async {
        tokio::signal::ctrl_c()
            .await
            .expect("install the SIGINT handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install the SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = interrupt => {}
        () = terminate => {}
    }

    tracing::info!("shutdown signal received; draining in-flight requests");
}
