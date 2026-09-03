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
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

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

    let router = build_router(AppState::new(db));
    if let Err(error) = axum::serve(listener, router).await {
        eprintln!("salt-server: server error: {error}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}
