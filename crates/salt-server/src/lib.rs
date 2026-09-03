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
//! This spec ships two unauthenticated routes, `GET /api/health` and `GET
//! /api/ready`, and the transport rules every later route inherits: the
//! error envelope, the `X-Salt-Request` mutation guard, the JSON body
//! limit, and the security headers. Sign-in, the authorization extractor and
//! bootstrap are issues #46, #47 and #48 — not started here.

mod config;
mod error;
mod request_id;
mod router;
mod state;

pub use config::{ConfigError, Environment, ServerConfig};
pub use error::ApiError;
pub use router::build_router;
pub use state::AppState;
