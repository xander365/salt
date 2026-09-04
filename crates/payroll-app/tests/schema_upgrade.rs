//! Proves the migrations upgrade a database that already holds rows, not only
//! an empty one. `#[sqlx::test]` applies every migration to a fresh database,
//! so it can never exercise a backfill: the table is empty when the column
//! arrives, and a broken backfill passes every other test in this crate while
//! failing the one deployment that matters — the real one.
//!
//! Each test here replays the migrations by hand up to the one under test,
//! writes the row a real deployment would already have, and then applies it.

use sqlx::PgPool;

/// Migration 0027 (issue #40) adds `employer.name`, a `NOT NULL` column, to a
/// table that already has rows in any running deployment.
const THE_NAME_MIGRATION: &str = "0027_an_employer_has_a_name.sql";
const THE_PERSON_MIGRATION: &str = "0031_person.sql";

/// Every migration, as `(file name, SQL)`, in the order the migrator applies
/// them. The version prefixes are zero-padded to a fixed width, so sorting the
/// names is sorting the versions.
fn migrations_in_order() -> Vec<(String, String)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");

    let mut migrations: Vec<(String, String)> = std::fs::read_dir(&dir)
        .expect("read the migrations directory")
        .map(|entry| entry.expect("read a migration entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
        .map(|path| {
            let name = path
                .file_name()
                .expect("a migration file has a name")
                .to_string_lossy()
                .into_owned();
            let sql = std::fs::read_to_string(&path)
                .unwrap_or_else(|err| panic!("read migration {name}: {err}"));
            (name, sql)
        })
        .collect();
    migrations.sort_by(|(left, _), (right, _)| left.cmp(right));

    assert!(
        !migrations.is_empty(),
        "expected the migrations directory to hold the schema, found nothing in {}",
        dir.display()
    );
    migrations
}

/// Applies one migration. `raw_sql` is what carries a file holding several
/// statements — and a `DO $$ ... $$` block — to PostgreSQL as written.
async fn apply(pool: &PgPool, (name, sql): &(String, String)) {
    sqlx::raw_sql(sql)
        .execute(pool)
        .await
        .unwrap_or_else(|err| panic!("applying migration {name} failed: {err}"));
}

/// The Employer that a deployment recorded before the name column existed
/// keeps its `PaySchedule`, its attribution and its id, and comes out of the
/// upgrade named — because `NOT NULL` on a populated table is only reachable
/// through a backfill, and a placeholder naming the row it stands for is what
/// keeps the upgrade lossless until an Owner can replace it (§0.2, §0.39).
#[sqlx::test(migrations = false)]
async fn an_employer_recorded_before_the_name_column_survives_the_upgrade(pool: PgPool) {
    let migrations = migrations_in_order();
    let name_migration = migrations
        .iter()
        .position(|(name, _)| name == THE_NAME_MIGRATION)
        .unwrap_or_else(|| panic!("this test names {THE_NAME_MIGRATION}, which no longer exists"));

    for migration in &migrations[..name_migration] {
        apply(&pool, migration).await;
    }

    sqlx::query(
        "INSERT INTO employer (id, period_end_day_kind, period_end_day_value, created_by)
         VALUES ('employer-1', 'day', 25, 'actor')",
    )
    .execute(&pool)
    .await
    .expect("record an Employer under the schema that had no name column");

    apply(&pool, &migrations[name_migration]).await;

    let (name, kind, value, created_by): (String, String, i16, String) = sqlx::query_as(
        "SELECT name, period_end_day_kind, period_end_day_value, created_by
         FROM employer WHERE id = 'employer-1'",
    )
    .fetch_one(&pool)
    .await
    .expect("the Employer recorded before the upgrade is still there");

    assert!(
        name.contains("employer-1"),
        "the backfilled name must name the Employer it stands for, got {name:?}"
    );
    assert!(
        !name.trim().is_empty(),
        "a backfilled name must not be blank, got {name:?}"
    );
    assert_eq!(
        (kind, value, created_by),
        ("day".to_owned(), 25, "actor".to_owned())
    );

    // The migrations after this one still apply on top of the upgraded row,
    // so the backfill leaves nothing for a later migration to trip over.
    for migration in &migrations[name_migration + 1..] {
        apply(&pool, migration).await;
    }
}

/// Migration 0031 gives an Employer-scoped Person to Employment rows that
/// predate the `person` table. A legacy `person_id` was only an opaque string:
/// it could be reused by several Employments of one Employer, or by different
/// Employers. The upgrade must preserve those relationships without letting a
/// same-looking id turn into one cross-Employer Person.
#[sqlx::test(migrations = false)]
async fn legacy_employments_gain_scoped_placeholder_people(pool: PgPool) {
    let migrations = migrations_in_order();
    let person_migration = migrations
        .iter()
        .position(|(name, _)| name == THE_PERSON_MIGRATION)
        .unwrap_or_else(|| {
            panic!("this test names {THE_PERSON_MIGRATION}, which no longer exists")
        });

    for migration in &migrations[..person_migration] {
        apply(&pool, migration).await;
    }

    sqlx::query(
        "INSERT INTO employer (id, name, period_end_day_kind, period_end_day_value, created_by)
         VALUES ('employer-1', 'Employer One', 'day', 25, 'actor'),
                ('employer-2', 'Employer Two', 'day', 25, 'actor')",
    )
    .execute(&pool)
    .await
    .expect("record Employers under the schema that had no person table");
    sqlx::query(
        "INSERT INTO employment (id, employer_id, person_id, start_date, created_by)
         VALUES ('employment-1', 'employer-1', 'legacy-person', '2026-01-01', 'actor'),
                ('employment-2', 'employer-1', 'legacy-person', '2026-02-01', 'actor-2'),
                ('employment-3', 'employer-2', 'legacy-person', '2026-01-01', 'actor')",
    )
    .execute(&pool)
    .await
    .expect("record Employments under the schema that had no person table");

    apply(&pool, &migrations[person_migration]).await;

    let rows: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT employment.id, employment.person_id, person.employer_id, person.full_name
         FROM employment
         JOIN person
           ON person.id = employment.person_id
          AND person.employer_id = employment.employer_id
         ORDER BY employment.id",
    )
    .fetch_all(&pool)
    .await
    .expect("the upgraded Employments each reference a Person of their Employer");

    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].2, "employer-1");
    assert_eq!(rows[1].2, "employer-1");
    assert_eq!(rows[2].2, "employer-2");
    assert_eq!(rows[0].1, rows[1].1, "one Employer reuses one Person row");
    assert_ne!(
        rows[0].1, rows[2].1,
        "different Employers receive distinct Person records"
    );
    for (_, _, _, full_name) in &rows {
        assert!(full_name.contains("legacy-person"));
        assert!(!full_name.trim().is_empty());
    }
}
