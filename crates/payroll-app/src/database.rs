//! `SaltDatabase`: the one opaque runtime handle every use case in this
//! crate takes in place of a bare `PgPool` (issue #39). Opening one is a
//! single operation that either yields a handle with the restricted role
//! already attached and the schema verified, or refuses as a whole — never
//! a pool a caller must remember to configure correctly afterwards.
//!
//! This is a pool type and a constructor, not a `Database` trait or a
//! repository seam: ADR-0009 already refused a seam with one
//! implementation, and that refusal stands.

use std::time::Duration;

use sqlx::migrate::Migrate;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{Executor, PgConnection, PgPool};

use crate::error::PayrollAppError;

/// The migrations compiled into this build, embedded at compile time from
/// `./migrations` (the same directory `#[sqlx::test]` and
/// `database_immutability.rs` already read). `connect` compares the
/// database's applied version against the highest version named here.
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!();

/// An opaque, already-authorized handle onto Salt's PostgreSQL database.
///
/// No accessor, `Deref`, or `Into` reaches the pool held inside — a crate
/// holding a `SaltDatabase` can pass it to a use case, and nothing else.
/// That is what makes it possible for a crate with no `sqlx` dependency at
/// all to hold and pass a Salt database, which is what makes the
/// `salt-server` "writes no SQL" rule implementable.
#[derive(Debug)]
pub struct SaltDatabase {
    pool: PgPool,
}

/// Plain configuration for [`SaltDatabase::connect`]. Every field is a
/// value, never an `sqlx` type, so assembling one — from environment
/// variables, say — needs no `sqlx` dependency either.
pub struct DatabaseConfig {
    pub url: String,
    pub max_connections: u32,
    pub acquire_timeout: Duration,
    pub idle_timeout: Option<Duration>,
}

impl SaltDatabase {
    /// Wraps an already-open pool. The only caller is a test: `#[sqlx::test]`
    /// hands one to every test, already pointed at a migrated, throwaway
    /// database, so there is nothing left for `connect` to do. Producing the
    /// argument needs `sqlx`, so this is the one place in the crate that
    /// does — not a hole in the seam `connect` otherwise guards.
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    pub(crate) fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Builds a pool against `config`, switches every connection in it onto
    /// the restricted `payroll_app` role, and refuses as a whole unless the
    /// database's applied migrations are at least the version compiled into
    /// this build.
    ///
    /// The role switch is installed as an `after_connect` hook rather than
    /// run once here: a hook runs again on every connection the pool ever
    /// opens, including a replacement opened later to refill the pool, so
    /// no use case can forget it and no later connection can escape it.
    pub async fn connect(config: &DatabaseConfig) -> Result<Self, PayrollAppError> {
        let connect_options: PgConnectOptions = config.url.parse()?;

        let pool = PgPoolOptions::new()
            .max_connections(config.max_connections)
            .acquire_timeout(config.acquire_timeout)
            .idle_timeout(config.idle_timeout)
            .after_connect(|conn, _meta| Box::pin(set_restricted_role(conn)))
            .connect_with(connect_options)
            .await?;

        // Refusing "as a whole" means leaving nothing behind: a pool whose
        // schema check fails is closed here rather than left for a drop to
        // reap, so a caller that retries does not stack up idle server-side
        // connections for every refusal.
        if let Err(refusal) = verify_schema_version(&pool).await {
            pool.close().await;
            return Err(refusal);
        }

        Ok(Self { pool })
    }
}

/// The `after_connect` hook itself (§6.2, §11, migration 0015), named so
/// both `connect` and this module's own tests exercise the identical
/// statement rather than two copies that could drift apart.
async fn set_restricted_role(conn: &mut PgConnection) -> Result<(), sqlx::Error> {
    conn.execute("SET ROLE payroll_app").await?;
    Ok(())
}

/// Refuses unless the database's applied migrations are at least the
/// version compiled into this build. Comparing versions rather than
/// applying migrations here: a schema behind this build's expectations is
/// refused as a whole, exactly as an ahead one is accepted as a whole —
/// `connect` never mutates the schema it is handed.
///
/// A database that has never been migrated at all has no migrations table
/// to read, which is the same refusal and not a different one, so it is
/// answered with the same typed variant rather than the raw "relation
/// does not exist" a bare `list_applied_migrations` would surface.
///
/// Turning this refusal into a refusal for `salt-server` to *start* is a
/// later ticket's job (issue #39's own note), not this function's.
async fn verify_schema_version(pool: &PgPool) -> Result<(), PayrollAppError> {
    let compiled = compiled_migration_version();

    let mut conn = pool.acquire().await?;
    let applied = applied_migration_version(&mut conn).await?;

    if applied.is_some_and(|applied| applied >= compiled) {
        return Ok(());
    }

    Err(PayrollAppError::SchemaOutOfDate { compiled, applied })
}

/// The highest migration version compiled into this build.
fn compiled_migration_version() -> i64 {
    MIGRATOR
        .iter()
        .map(|migration| migration.version)
        .max()
        .expect("payroll-app ships at least one migration")
}

/// The highest migration version applied to the database, or `None` when
/// none has been — including when the migrations table itself is absent,
/// which is what a never-migrated database looks like. The table is looked
/// up first rather than letting the read fail, so "never migrated" stays a
/// version answer and does not become a database error.
async fn applied_migration_version(
    conn: &mut PgConnection,
) -> Result<Option<i64>, PayrollAppError> {
    let (migrations_table_exists,): (bool,) =
        sqlx::query_as("SELECT to_regclass('_sqlx_migrations') IS NOT NULL")
            .fetch_one(&mut *conn)
            .await?;
    if !migrations_table_exists {
        return Ok(None);
    }

    Ok(conn
        .list_applied_migrations()
        .await
        .map_err(|err| PayrollAppError::Database(err.to_string()))?
        .into_iter()
        .map(|migration| migration.version)
        .max())
}

#[cfg(test)]
mod tests {
    use super::*;

    const INSUFFICIENT_PRIVILEGE: &str = "42501";

    /// The `DatabaseConfig` that reaches the ephemeral database a
    /// `#[sqlx::test]` just migrated: the suite's own `DATABASE_URL` with
    /// its database name swapped for this test's. Going the long way round
    /// through a URL string is the point — it is the only way a test can
    /// exercise [`SaltDatabase::connect`] itself rather than a hand-built
    /// copy of it, and a copy is exactly what would let the real
    /// constructor drift out from under these assertions.
    fn config_for(pool: &PgPool) -> DatabaseConfig {
        let database = (*pool.connect_options())
            .clone()
            .get_database()
            .expect("a #[sqlx::test] pool names its throwaway database")
            .to_string();
        let suite_url = std::env::var("DATABASE_URL")
            .expect("the test suite is run with DATABASE_URL set (see AGENTS.md)");

        let (authority, query) = match suite_url.split_once('?') {
            Some((authority, query)) => (authority, Some(query)),
            None => (suite_url.as_str(), None),
        };
        let without_database = authority
            .rsplit_once('/')
            .expect("a postgres URL names a database after its final '/'")
            .0;

        let mut url = format!("{without_database}/{database}");
        if let Some(query) = query {
            url.push('?');
            url.push_str(query);
        }

        DatabaseConfig {
            url,
            max_connections: 2,
            acquire_timeout: Duration::from_secs(10),
            idle_timeout: None,
        }
    }

    /// Prior art: `database_immutability.rs`'s own proof of the same
    /// refusal, which is why every assertion there runs after `SET ROLE
    /// payroll_app` too — proving it as the migration-applying owner role
    /// would pass while proving nothing.
    #[sqlx::test]
    async fn an_update_on_finalized_payroll_issued_through_a_salt_database_is_refused(
        pool: PgPool,
    ) {
        sqlx::query(
            "INSERT INTO employer (id, period_end_day_kind, period_end_day_value, created_by)
             VALUES ('employer-1', 'day', 25, 'test-actor')",
        )
        .execute(&pool)
        .await
        .expect("insert employer");
        sqlx::query(
            "INSERT INTO employment (id, employer_id, person_id, start_date, created_by)
             VALUES ('employment-1', 'employer-1', 'person-1', '2026-03-01', 'test-actor')",
        )
        .execute(&pool)
        .await
        .expect("insert employment");
        let run_id: (String,) = sqlx::query_as(
            "INSERT INTO payroll_run
                (employer_id, period_start, period_end, pay_date, kind, status, created_by)
             VALUES
                ('employer-1', '2026-03-01', '2026-03-31', '2026-04-05', 'ordinary', 'finalized',
                 'test-actor')
             RETURNING id::text",
        )
        .fetch_one(&pool)
        .await
        .expect("insert payroll_run");
        sqlx::query(
            "INSERT INTO payroll_run_employment (payroll_run_id, employment_id)
             VALUES ($1::uuid, 'employment-1')",
        )
        .bind(&run_id.0)
        .execute(&pool)
        .await
        .expect("insert run membership");
        let finalized_id: (String,) = sqlx::query_as(
            "INSERT INTO finalized_payroll
                (payroll_run_id, employment_id, employer_id, period_start, period_end, tax_year,
                 payroll_input_json, payroll_rules_json, payroll_calculation_json,
                 taxable_remuneration, paye, paye_table_id, ssc_rules_id, salt_version,
                 finalized_by)
             VALUES
                ($1::uuid, 'employment-1', 'employer-1', '2026-03-01', '2026-03-31', 2026,
                 '{}', '{}', '{}', 15000.00, 1200.00, 'paye-1', 'ssc-1', '0.1.0+gdeadbeef',
                 'test-actor')
             RETURNING id::text",
        )
        .bind(&run_id.0)
        .fetch_one(&pool)
        .await
        .expect("insert finalized_payroll");

        let db = SaltDatabase::connect(&config_for(&pool))
            .await
            .expect("connect against the migrated test database");

        let result = sqlx::query("UPDATE finalized_payroll SET paye = 0 WHERE id = $1::uuid")
            .bind(&finalized_id.0)
            .execute(db.pool())
            .await;

        let err = result.expect_err("a SaltDatabase's UPDATE on finalized_payroll must be refused");
        assert!(
            matches!(&err, sqlx::Error::Database(db_err) if db_err.code().as_deref() == Some(INSUFFICIENT_PRIVILEGE)),
            "expected an insufficient_privilege refusal, got {err:?}"
        );
    }

    #[sqlx::test]
    async fn connect_refuses_a_database_behind_the_compiled_migration_version(pool: PgPool) {
        let compiled = compiled_migration_version();

        sqlx::query("DELETE FROM _sqlx_migrations WHERE version = $1")
            .bind(compiled)
            .execute(&pool)
            .await
            .expect("roll the applied version back for this test");

        let err = SaltDatabase::connect(&config_for(&pool))
            .await
            .expect_err("a database behind the compiled version must be refused");

        match err {
            PayrollAppError::SchemaOutOfDate {
                compiled: refused_compiled,
                applied,
            } => {
                assert_eq!(refused_compiled, compiled);
                assert!(
                    applied.is_none_or(|applied| applied < compiled),
                    "expected the applied version to be behind {compiled}, got {applied:?}"
                );
            }
            other => panic!("expected SchemaOutOfDate, got {other:?}"),
        }
    }

    /// The refusal a fresh, never-migrated database earns. It is the same
    /// refusal as "behind", not a different one, so it must arrive as the
    /// same typed variant rather than as the raw "relation does not exist"
    /// that reading a missing migrations table would otherwise produce.
    #[sqlx::test]
    async fn connect_refuses_a_database_that_has_never_been_migrated(pool: PgPool) {
        sqlx::query("DROP TABLE _sqlx_migrations")
            .execute(&pool)
            .await
            .expect("drop the migrations table for this test");

        let err = SaltDatabase::connect(&config_for(&pool))
            .await
            .expect_err("a never-migrated database must be refused");

        assert!(
            matches!(
                err,
                PayrollAppError::SchemaOutOfDate {
                    compiled,
                    applied: None,
                } if compiled == compiled_migration_version()
            ),
            "expected SchemaOutOfDate with no applied version, got {err:?}"
        );
    }

    #[tokio::test]
    async fn connect_refuses_a_url_that_does_not_parse() {
        let config = DatabaseConfig {
            url: "not a postgres url".to_string(),
            max_connections: 1,
            acquire_timeout: Duration::from_secs(1),
            idle_timeout: None,
        };

        let err = SaltDatabase::connect(&config)
            .await
            .expect_err("an unparseable url must be refused");
        assert!(matches!(err, PayrollAppError::Database(_)));
    }
}
