//! Configuration read once at startup, from environment variables only, into
//! one struct that refuses to build on a missing or malformed value — issue
//! #45's own acceptance criteria.
//!
//! Parsing is factored through [`ServerConfig::from_source`] rather than
//! reading `std::env` directly, so a test can supply a private map instead
//! of mutating the process's real environment — `std::env::set_var` is
//! global and races across tests run in parallel, and this sidesteps that
//! without weakening what [`ServerConfig::from_env`] does in production.

use std::fmt;
use std::net::SocketAddr;
use std::time::Duration;

use payroll_app::DatabaseConfig;

/// Whether this process is a production deployment. The one thing this
/// distinction gates today is [`ServerConfig::insecure_cookies`]: a
/// production process may never disable the `Secure` cookie attribute
/// (issue #45's own acceptance criteria; the attribute itself belongs to a
/// later spec's session cookie).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Environment {
    Development,
    Production,
}

/// Plain values [`crate::state::AppState`] and the binary's `main` are built
/// from. Every field here was named by an acceptance criterion or a Deep
/// Instruction in issue #45 — nothing is speculative.
pub struct ServerConfig {
    pub database: DatabaseConfig,
    pub bind_addr: SocketAddr,
    pub environment: Environment,
    /// The development flag that disables the session cookie's `Secure`
    /// attribute. No route in this spec sets a cookie; the flag is modelled
    /// and validated here because refusing an unsafe combination is this
    /// crate's job at the one place configuration is assembled, not the
    /// later spec's that first reads it.
    pub insecure_cookies: bool,
}

/// [`ServerConfig`]'s own `Debug`, written by hand rather than derived:
/// [`DatabaseConfig`] carries the database URL — which can embed a password —
/// and has no `Debug` of its own, so a derive here could not compile without
/// first exposing that field's `Debug` unredacted somewhere. This is the
/// data a startup log line, or a panic message, is allowed to repeat.
impl fmt::Debug for ServerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServerConfig")
            .field("database_url", &"<redacted>")
            .field("database_max_connections", &self.database.max_connections)
            .field("database_acquire_timeout", &self.database.acquire_timeout)
            .field("database_idle_timeout", &self.database.idle_timeout)
            .field("bind_addr", &self.bind_addr)
            .field("environment", &self.environment)
            .field("insecure_cookies", &self.insecure_cookies)
            .finish()
    }
}

/// Why [`ServerConfig::from_source`] refused to build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// `var` was absent, or present but empty or only whitespace.
    Missing(&'static str),
    /// `var` was present but could not be parsed as the value it names.
    Invalid {
        var: &'static str,
        value: String,
        reason: String,
    },
    /// [`ServerConfig::insecure_cookies`] was set alongside
    /// [`Environment::Production`] — the one combination this spec's own
    /// acceptance criteria names as a refusal to *start*, not merely a
    /// refusal to trust the cookie.
    InsecureCookiesInProduction,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(var) => write!(f, "{var} is required but was not set"),
            Self::Invalid { var, value, reason } => {
                write!(f, "{var} is set to \"{value}\", which is invalid: {reason}")
            }
            Self::InsecureCookiesInProduction => write!(
                f,
                "SALT_INSECURE_COOKIES cannot be set while SALT_ENVIRONMENT=production: a \
                 production server must never disable the session cookie's Secure attribute"
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

const DEFAULT_BIND_ADDR_STR: &str = "0.0.0.0:8080";
const DEFAULT_MAX_CONNECTIONS: u32 = 10;
const DEFAULT_ACQUIRE_TIMEOUT_SECS: u64 = 10;

fn default_bind_addr() -> SocketAddr {
    DEFAULT_BIND_ADDR_STR
        .parse()
        .expect("DEFAULT_BIND_ADDR_STR must itself be a valid socket address")
}

impl ServerConfig {
    /// Reads every variable from the process environment. The only
    /// constructor `salt-server`'s binary calls; tests use
    /// [`Self::from_source`] directly instead.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_source(|key| std::env::var(key).ok())
    }

    /// Builds a [`ServerConfig`] from `get`, a lookup standing in for the
    /// environment. Refuses as a whole — via `?` on the first problem found
    /// — rather than collecting every problem, which is what "read once at
    /// startup" (issue #45) demands: a caller either has a config it can run
    /// with, or an error naming one thing to fix first.
    pub fn from_source(get: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let url = required(&get, "DATABASE_URL")?;
        let environment = parse_environment(&get)?;
        let bind_addr = parsed_or_default(&get, "SALT_BIND_ADDR", default_bind_addr())?;
        let max_connections = parsed_or_default(
            &get,
            "SALT_DATABASE_MAX_CONNECTIONS",
            DEFAULT_MAX_CONNECTIONS,
        )?;
        let acquire_timeout_secs: u64 = parsed_or_default(
            &get,
            "SALT_DATABASE_ACQUIRE_TIMEOUT_SECS",
            DEFAULT_ACQUIRE_TIMEOUT_SECS,
        )?;
        let idle_timeout_secs: Option<u64> =
            parsed_optional(&get, "SALT_DATABASE_IDLE_TIMEOUT_SECS")?;

        // Each of these parses cleanly and then makes the process unable to
        // serve a single request: a pool of zero connections never hands one
        // out, and a zero acquire or idle timeout expires the instant it is
        // armed. "Refuses on a malformed value" (issue #45) has to mean
        // refusing a value that is well-formed and unusable too, or the typo
        // is found at 3am against a server that started happily.
        nonzero(max_connections, "SALT_DATABASE_MAX_CONNECTIONS")?;
        nonzero(acquire_timeout_secs, "SALT_DATABASE_ACQUIRE_TIMEOUT_SECS")?;
        if let Some(idle_timeout_secs) = idle_timeout_secs {
            nonzero(idle_timeout_secs, "SALT_DATABASE_IDLE_TIMEOUT_SECS")?;
        }
        let insecure_cookies = parsed_bool_or_default(&get, "SALT_INSECURE_COOKIES", false)?;

        if insecure_cookies && environment == Environment::Production {
            return Err(ConfigError::InsecureCookiesInProduction);
        }

        Ok(Self {
            database: DatabaseConfig {
                url,
                max_connections,
                acquire_timeout: Duration::from_secs(acquire_timeout_secs),
                idle_timeout: idle_timeout_secs.map(Duration::from_secs),
            },
            bind_addr,
            environment,
            insecure_cookies,
        })
    }
}

/// Refuses a numeric setting of zero. Kept as its own step rather than
/// folded into parsing, because zero is a perfectly well-formed number —
/// what is wrong with it is what it means to the pool, not its syntax.
fn nonzero(value: impl Into<u64>, var: &'static str) -> Result<(), ConfigError> {
    let value = value.into();
    if value == 0 {
        return Err(ConfigError::Invalid {
            var,
            value: value.to_string(),
            reason: "must be greater than zero".to_string(),
        });
    }
    Ok(())
}

fn required(
    get: &impl Fn(&str) -> Option<String>,
    var: &'static str,
) -> Result<String, ConfigError> {
    match get(var) {
        Some(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(ConfigError::Missing(var)),
    }
}

fn parse_environment(get: &impl Fn(&str) -> Option<String>) -> Result<Environment, ConfigError> {
    let value = required(get, "SALT_ENVIRONMENT")?;
    match value.as_str() {
        "production" => Ok(Environment::Production),
        "development" => Ok(Environment::Development),
        _ => Err(ConfigError::Invalid {
            var: "SALT_ENVIRONMENT",
            value,
            reason: "must be exactly \"production\" or \"development\"".to_string(),
        }),
    }
}

/// `var`'s value parsed as `T`, or `default` when `var` is unset or blank.
/// A `var` that is set to something non-blank but unparsable is refused
/// rather than falling back — a typo in a value the caller bothered to
/// state is exactly the "3am surprise" (issue #45's own instruction) startup
/// validation exists to catch instead of silently overriding.
fn parsed_or_default<T: std::str::FromStr>(
    get: &impl Fn(&str) -> Option<String>,
    var: &'static str,
    default: T,
) -> Result<T, ConfigError> {
    match get(var) {
        Some(value) if !value.trim().is_empty() => {
            value.trim().parse().map_err(|_| ConfigError::Invalid {
                var,
                value,
                reason: "could not be parsed".to_string(),
            })
        }
        _ => Ok(default),
    }
}

fn parsed_optional<T: std::str::FromStr>(
    get: &impl Fn(&str) -> Option<String>,
    var: &'static str,
) -> Result<Option<T>, ConfigError> {
    match get(var) {
        Some(value) if !value.trim().is_empty() => {
            value
                .trim()
                .parse()
                .map(Some)
                .map_err(|_| ConfigError::Invalid {
                    var,
                    value,
                    reason: "could not be parsed".to_string(),
                })
        }
        _ => Ok(None),
    }
}

fn parsed_bool_or_default(
    get: &impl Fn(&str) -> Option<String>,
    var: &'static str,
    default: bool,
) -> Result<bool, ConfigError> {
    match get(var) {
        Some(value) if !value.trim().is_empty() => match value.trim() {
            "1" | "true" => Ok(true),
            "0" | "false" => Ok(false),
            _ => Err(ConfigError::Invalid {
                var,
                value,
                reason: "must be \"true\"/\"1\" or \"false\"/\"0\"".to_string(),
            }),
        },
        _ => Ok(default),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn source(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |key| map.get(key).cloned()
    }

    fn minimal_valid_pairs() -> Vec<(&'static str, &'static str)> {
        vec![
            ("DATABASE_URL", "postgres://localhost/salt"),
            ("SALT_ENVIRONMENT", "development"),
        ]
    }

    #[test]
    fn a_minimal_environment_builds_with_sensible_defaults() {
        let config = ServerConfig::from_source(source(&minimal_valid_pairs()))
            .expect("a minimal, valid environment must build");

        assert_eq!(config.environment, Environment::Development);
        assert_eq!(config.bind_addr, default_bind_addr());
        assert_eq!(config.database.max_connections, DEFAULT_MAX_CONNECTIONS);
        assert!(!config.insecure_cookies);
    }

    #[test]
    fn a_missing_database_url_is_refused() {
        let err = ServerConfig::from_source(source(&[("SALT_ENVIRONMENT", "development")]))
            .expect_err("DATABASE_URL is required");

        assert_eq!(err, ConfigError::Missing("DATABASE_URL"));
    }

    #[test]
    fn a_blank_database_url_is_refused_the_same_as_a_missing_one() {
        let err = ServerConfig::from_source(source(&[
            ("DATABASE_URL", "   "),
            ("SALT_ENVIRONMENT", "development"),
        ]))
        .expect_err("a blank DATABASE_URL is refused");

        assert_eq!(err, ConfigError::Missing("DATABASE_URL"));
    }

    #[test]
    fn a_missing_environment_is_refused() {
        let err =
            ServerConfig::from_source(source(&[("DATABASE_URL", "postgres://localhost/salt")]))
                .expect_err("SALT_ENVIRONMENT is required");

        assert_eq!(err, ConfigError::Missing("SALT_ENVIRONMENT"));
    }

    #[test]
    fn an_unrecognised_environment_value_is_refused() {
        let mut pairs = minimal_valid_pairs();
        pairs.push(("SALT_ENVIRONMENT", "prod"));
        let err = ServerConfig::from_source(source(&pairs))
            .expect_err("only \"production\" or \"development\" are recognised");

        assert!(matches!(
            err,
            ConfigError::Invalid {
                var: "SALT_ENVIRONMENT",
                ..
            }
        ));
    }

    #[test]
    fn a_malformed_bind_addr_is_refused_rather_than_defaulted() {
        let mut pairs = minimal_valid_pairs();
        pairs.push(("SALT_BIND_ADDR", "not-an-address"));
        let err = ServerConfig::from_source(source(&pairs))
            .expect_err("a stated but unparsable SALT_BIND_ADDR must be refused");

        assert!(matches!(
            err,
            ConfigError::Invalid {
                var: "SALT_BIND_ADDR",
                ..
            }
        ));
    }

    #[test]
    fn a_malformed_max_connections_is_refused() {
        let mut pairs = minimal_valid_pairs();
        pairs.push(("SALT_DATABASE_MAX_CONNECTIONS", "many"));
        let err = ServerConfig::from_source(source(&pairs))
            .expect_err("a non-numeric SALT_DATABASE_MAX_CONNECTIONS must be refused");

        assert!(matches!(
            err,
            ConfigError::Invalid {
                var: "SALT_DATABASE_MAX_CONNECTIONS",
                ..
            }
        ));
    }

    #[test]
    fn a_zero_max_connections_is_refused_although_it_parses() {
        let mut pairs = minimal_valid_pairs();
        pairs.push(("SALT_DATABASE_MAX_CONNECTIONS", "0"));
        let err = ServerConfig::from_source(source(&pairs))
            .expect_err("a pool of zero connections can never serve a request");

        assert!(matches!(
            err,
            ConfigError::Invalid {
                var: "SALT_DATABASE_MAX_CONNECTIONS",
                ..
            }
        ));
    }

    #[test]
    fn a_zero_acquire_timeout_is_refused() {
        let mut pairs = minimal_valid_pairs();
        pairs.push(("SALT_DATABASE_ACQUIRE_TIMEOUT_SECS", "0"));
        let err = ServerConfig::from_source(source(&pairs))
            .expect_err("a zero acquire timeout expires before it can succeed");

        assert!(matches!(
            err,
            ConfigError::Invalid {
                var: "SALT_DATABASE_ACQUIRE_TIMEOUT_SECS",
                ..
            }
        ));
    }

    #[test]
    fn a_zero_idle_timeout_is_refused_while_an_absent_one_is_fine() {
        let mut pairs = minimal_valid_pairs();
        pairs.push(("SALT_DATABASE_IDLE_TIMEOUT_SECS", "0"));
        let err = ServerConfig::from_source(source(&pairs))
            .expect_err("a zero idle timeout closes every connection immediately");

        assert!(matches!(
            err,
            ConfigError::Invalid {
                var: "SALT_DATABASE_IDLE_TIMEOUT_SECS",
                ..
            }
        ));

        let config = ServerConfig::from_source(source(&minimal_valid_pairs()))
            .expect("an absent idle timeout is not the same as a zero one");
        assert_eq!(config.database.idle_timeout, None);
    }

    #[test]
    fn insecure_cookies_alone_in_development_is_accepted() {
        let mut pairs = minimal_valid_pairs();
        pairs.push(("SALT_INSECURE_COOKIES", "true"));
        let config = ServerConfig::from_source(source(&pairs))
            .expect("insecure cookies are fine in development");

        assert!(config.insecure_cookies);
    }

    #[test]
    fn insecure_cookies_combined_with_production_is_refused() {
        let pairs = [
            ("DATABASE_URL", "postgres://localhost/salt"),
            ("SALT_ENVIRONMENT", "production"),
            ("SALT_INSECURE_COOKIES", "true"),
        ];
        let err = ServerConfig::from_source(source(&pairs))
            .expect_err("insecure cookies in production must refuse to build");

        assert_eq!(err, ConfigError::InsecureCookiesInProduction);
    }

    #[test]
    fn production_without_insecure_cookies_is_accepted() {
        let pairs = [
            ("DATABASE_URL", "postgres://localhost/salt"),
            ("SALT_ENVIRONMENT", "production"),
        ];
        let config = ServerConfig::from_source(source(&pairs)).expect("production alone is fine");

        assert_eq!(config.environment, Environment::Production);
        assert!(!config.insecure_cookies);
    }

    #[test]
    fn debug_output_redacts_the_database_url() {
        let config = ServerConfig::from_source(source(&minimal_valid_pairs()))
            .expect("a minimal, valid environment must build");

        let debug = format!("{config:?}");

        assert!(!debug.contains("postgres://localhost/salt"));
        assert!(debug.contains("<redacted>"));
    }
}
