//! The thin binary: parses configuration, connects to the database and
//! listens; or, given a `bootstrap` subcommand, parses that command's own
//! arguments, connects, and calls `payroll_app::bootstrap` once. Every
//! decision beyond argument parsing — the router, the transport rules, the
//! routes themselves, and everything bootstrap actually creates — lives in
//! a library so a test can build the identical thing in-process (`lib.rs`,
//! `payroll_app::bootstrap`).
//!
//! Refuses to start, with a message on stderr and a non-zero exit, on a
//! missing or malformed configuration value, on an unsafe configuration
//! combination, or on a database whose schema is behind this build (issue
//! #45's own acceptance criteria). None of those are things to keep running
//! and hope about — `run_bootstrap` holds the same discipline for its own
//! refusals.

use std::process::ExitCode;

use payroll_app::SaltDatabase;
use salt_server::{AppState, BootstrapArgs, ServerConfig, build_router, parse_bootstrap_args};

#[tokio::main]
async fn main() -> ExitCode {
    let mut argv = std::env::args();
    argv.next(); // the binary's own path — argument parsing never reads it.

    match argv.next().as_deref() {
        Some("bootstrap") => run_bootstrap(&argv.collect::<Vec<_>>()).await,
        Some(other) => {
            eprintln!("salt-server: unrecognized argument: {other}");
            ExitCode::FAILURE
        }
        None => run_server().await,
    }
}

async fn run_server() -> ExitCode {
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

/// Reads the Operator's password from stdin, with typed characters hidden
/// when stdin is an interactive terminal. When it is not — piped input, the
/// normal shape for a scripted or containerized bootstrap — a plain line is
/// read instead, since there is no terminal for `rpassword` to hide
/// anything on and it refuses outright rather than fall back on its own.
///
/// The prompt itself goes to stderr, not stdout: stdout carries the one line
/// this command is read for — the ids it created — and a script capturing it
/// should not have to strip a prompt out of the front of it.
fn read_password() -> std::io::Result<String> {
    use std::io::{IsTerminal, Write};

    if std::io::stdin().is_terminal() {
        return rpassword::prompt_password("Operator password: ");
    }

    eprint!("Operator password: ");
    std::io::stderr().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim_end_matches(['\n', '\r']).to_string())
}

/// `salt-server bootstrap`'s own body: parse `args`, read the password from
/// stdin (never a flag — issue #48's own acceptance criterion), connect, and
/// call `payroll_app::bootstrap` exactly once. No route and no HTTP server
/// is involved.
async fn run_bootstrap(args: &[String]) -> ExitCode {
    let parsed: BootstrapArgs = match parse_bootstrap_args(args) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("salt-server bootstrap: {error}");
            return ExitCode::FAILURE;
        }
    };

    let config = match ServerConfig::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("salt-server bootstrap: refusing to run: {error}");
            return ExitCode::FAILURE;
        }
    };

    let db = match SaltDatabase::connect(&config.database).await {
        Ok(db) => db,
        Err(error) => {
            eprintln!("salt-server bootstrap: refusing to run: {error}");
            return ExitCode::FAILURE;
        }
    };

    // The only place the password is ever read — it reaches
    // `payroll_app::bootstrap` and nowhere else.
    let password = match read_password() {
        Ok(password) => password,
        Err(error) => {
            eprintln!("salt-server bootstrap: could not read the password from stdin: {error}");
            return ExitCode::FAILURE;
        }
    };

    match payroll_app::bootstrap(
        &db,
        &parsed.email,
        &parsed.display_name,
        &password,
        &parsed.employer_name,
        parsed.period_end_day,
    )
    .await
    {
        Ok(outcome) => {
            println!(
                "salt-server bootstrap: created Operator {} and Employer {}",
                outcome.operator_id, outcome.employer_id
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("salt-server bootstrap: {error}");
            ExitCode::FAILURE
        }
    }
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
