//! `salt-server`: the Axum delivery crate (issue #45, parent #38 Spec 1 of
//! 3). A library plus a thin binary — the library builds the real router so
//! an in-process test drives it with no TCP port and no browser
//! (`tests/router.rs`); the binary only parses configuration, connects to
//! the database and listens.
//!
//! This manifest carries no `sqlx` (ADR-0018). Every query this crate needs
//! goes through `payroll-app`'s `SaltDatabase` and its use cases — nothing
//! here writes SQL.
//!
//! This crate ships `GET /api/health` and `GET /api/ready` (issue #45), the
//! transport rules every later route inherits — the error envelope, the
//! `X-Salt-Request` mutation guard, the JSON body limit, and the security
//! headers — the three session routes (issue #46): `POST`, `DELETE` and
//! `GET /api/session`; from issue #47, the `AuthorizedEmployerContext`
//! extractor (`authorized_employer`) and `GET /api/employers` (`employers`);
//! and, from issue #48, `bootstrap_cli`'s argument parsing for
//! `salt-server bootstrap` — a CLI command, not an HTTP route, so it ships
//! no route of its own and its actual work is `payroll_app::bootstrap`.
//!
//! From issue #50: `payroll_error` maps every `PayrollAppError` and
//! `PayrollError` variant to a status, a stable `code` and `details`, ahead
//! of the payroll routes that lean on it — this ticket ships none itself.

mod authorized_employer;
mod bootstrap_cli;
mod config;
mod employer_particulars;
mod employers;
mod employment_facts;
mod employments;
mod error;
mod finalized_payroll;
mod payroll_error;
mod payroll_runs;
mod person_particulars;
mod request_id;
mod router;
mod session;
mod state;

pub use bootstrap_cli::{BootstrapArgs, BootstrapArgsError, parse as parse_bootstrap_args};
pub use config::{ConfigError, Environment, ServerConfig};
pub use error::ApiError;
pub use router::build_router;
pub use state::AppState;

/// ADR-0018's invariant, and the shortest thing a reviewer of issue #45
/// checks: `salt-server`'s manifest carries no `sqlx`. Asserted here rather
/// than left to review, because a review catches it once and a test catches
/// it every time. The manifest is read at compile time, so this needs no
/// file access and cannot go looking at the wrong path.
#[cfg(test)]
mod manifest {
    const MANIFEST: &str = include_str!("../Cargo.toml");

    /// Only dependency *names* are examined: the word "sqlx" appears in this
    /// manifest's comments on purpose, explaining why it is not a
    /// dependency, and a plain substring search would fail on the very
    /// comment that documents the rule.
    fn declared_dependency_names() -> Vec<String> {
        let mut names = Vec::new();
        let mut in_dependencies = false;
        for line in MANIFEST.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_dependencies = line.contains("dependencies");
                continue;
            }
            if !in_dependencies || line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((name, _)) = line.split_once('=') {
                names.push(name.trim().trim_matches('"').to_string());
            }
        }
        names
    }

    #[test]
    fn no_dependency_named_sqlx_is_declared() {
        let names = declared_dependency_names();
        assert!(
            !names.iter().any(|name| name == "sqlx"),
            "ADR-0018: salt-server must reach PostgreSQL only through \
             payroll-app's SaltDatabase, but its manifest declares sqlx. \
             Declared dependencies: {names:?}"
        );
    }

    /// Guards the guard: a parser that found nothing would pass the test
    /// above for the wrong reason.
    #[test]
    fn the_manifest_parser_finds_the_dependencies_that_are_there() {
        let names = declared_dependency_names();
        assert!(names.iter().any(|name| name == "payroll-app"), "{names:?}");
        assert!(names.iter().any(|name| name == "axum"), "{names:?}");
    }
}
