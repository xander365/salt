//! Proves the use cases issue #44 introduces: `create_session`,
//! `load_session` and `delete_session` — reached through the public API a
//! later ticket's HTTP layer calls, not raw SQL.

use chrono::{DateTime, Duration, Utc};
use payroll_app::{
    OperatorId, SaltDatabase, create_operator, create_session, delete_session, load_session,
};
use sqlx::PgPool;

async fn an_operator(db: &SaltDatabase) -> OperatorId {
    create_operator(
        db,
        "alice@example.com",
        "Alice",
        "correct horse battery staple",
    )
    .await
    .unwrap()
}

#[sqlx::test]
async fn a_session_is_created_and_loaded_by_its_own_token(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let operator_id = an_operator(&db).await;
    let now = Utc::now();

    let created = create_session(&db, &operator_id, now).await.unwrap();

    let loaded = load_session(&db, &created.token, now)
        .await
        .unwrap()
        .expect("the session just created must load on its own token");

    assert_eq!(loaded.id, created.id);
    assert_eq!(loaded.operator_id, operator_id);
}

#[sqlx::test]
async fn an_unknown_token_does_not_load(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);

    let loaded = load_session(&db, "not-a-real-token", Utc::now())
        .await
        .unwrap();

    assert_eq!(loaded, None);
}

#[sqlx::test]
async fn a_session_past_the_absolute_timer_does_not_load_even_when_recently_seen(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let operator_id = an_operator(&db).await;
    let created_at = Utc::now();
    let created = create_session(&db, &operator_id, created_at).await.unwrap();

    // Just past twelve hours, even though the idle window alone would still
    // be wide open.
    let now = created_at + Duration::hours(12) + Duration::seconds(1);

    let loaded = load_session(&db, &created.token, now).await.unwrap();

    assert_eq!(loaded, None);
}

#[sqlx::test]
async fn a_session_past_the_idle_timer_does_not_load_even_within_the_absolute_window(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let operator_id = an_operator(&db).await;
    let created_at = Utc::now();
    let created = create_session(&db, &operator_id, created_at).await.unwrap();

    // Just past eight hours idle, still well inside the twelve-hour absolute
    // deadline.
    let now = created_at + Duration::hours(8) + Duration::seconds(1);

    let loaded = load_session(&db, &created.token, now).await.unwrap();

    assert_eq!(loaded, None);
}

#[sqlx::test]
async fn a_lookup_that_finds_an_expired_session_deletes_it(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let operator_id = an_operator(&db).await;
    let created_at = Utc::now();
    let created = create_session(&db, &operator_id, created_at).await.unwrap();

    let expired_at = created_at + Duration::hours(12) + Duration::seconds(1);
    load_session(&db, &created.token, expired_at).await.unwrap();

    let row_count: (i64,) = sqlx::query_as("SELECT count(*) FROM session")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        row_count.0, 0,
        "the expired row must have been deleted by the lookup that found it"
    );
}

#[sqlx::test]
async fn last_seen_at_is_extended_only_once_it_is_more_than_five_minutes_stale(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let operator_id = an_operator(&db).await;
    let created_at = Utc::now();
    let created = create_session(&db, &operator_id, created_at).await.unwrap();

    // Fresh: within five minutes, so last_seen_at is left alone.
    let still_fresh = created_at + Duration::minutes(4);
    load_session(&db, &created.token, still_fresh)
        .await
        .unwrap();

    let after_fresh_read = last_seen_at(&pool, &created.id).await;
    assert_eq!(
        after_fresh_read.timestamp(),
        created_at.timestamp(),
        "last_seen_at must not move while it is fresh"
    );

    // Stale: more than five minutes since last_seen_at, so it is extended to
    // this read's own time.
    let now_stale = created_at + Duration::minutes(6);
    load_session(&db, &created.token, now_stale).await.unwrap();

    let after_stale_read = last_seen_at(&pool, &created.id).await;
    assert_eq!(after_stale_read.timestamp(), now_stale.timestamp());
}

async fn last_seen_at(pool: &PgPool, session_id: &payroll_app::SessionId) -> DateTime<Utc> {
    let (last_seen_at,): (DateTime<Utc>,) =
        sqlx::query_as("SELECT last_seen_at FROM session WHERE id = $1::uuid")
            .bind(session_id.as_str())
            .fetch_one(pool)
            .await
            .unwrap();
    last_seen_at
}

#[sqlx::test]
async fn a_deleted_session_no_longer_loads(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let operator_id = an_operator(&db).await;
    let now = Utc::now();
    let created = create_session(&db, &operator_id, now).await.unwrap();

    delete_session(&db, &created.id).await.unwrap();

    let loaded = load_session(&db, &created.token, now).await.unwrap();
    assert_eq!(loaded, None);
}

#[sqlx::test]
async fn deleting_an_already_deleted_session_is_not_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let operator_id = an_operator(&db).await;
    let created = create_session(&db, &operator_id, Utc::now()).await.unwrap();

    delete_session(&db, &created.id).await.unwrap();
    delete_session(&db, &created.id).await.unwrap();
}

#[sqlx::test]
async fn creating_a_session_for_a_disabled_operator_still_succeeds(pool: PgPool) {
    // Session creation authenticates nothing about the Operator's own
    // status — it only records that a session now exists for this identity.
    // Whether a disabled Operator's session is honoured is a later spec's
    // concern (ADR-0017's joined authorization check), not this table's.
    let db = SaltDatabase::from_pool(pool);
    let operator_id = an_operator(&db).await;
    payroll_app::disable_operator(&db, &operator_id)
        .await
        .unwrap();

    let result = create_session(&db, &operator_id, Utc::now()).await;

    assert!(result.is_ok());
}
