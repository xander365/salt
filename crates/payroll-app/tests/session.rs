//! Proves the use cases issue #44 introduces: `create_session`,
//! `load_session` and `delete_session` — reached through the public API a
//! later ticket's HTTP layer calls, not raw SQL.

use chrono::{DateTime, Duration, SubsecRound, Utc};
use payroll_app::{
    OperatorId, SaltDatabase, clear_expired_sessions, create_operator, create_session,
    create_session_for_active_operator, delete_session, load_session,
};
use sqlx::PgPool;

/// A clock reading at PostgreSQL's own precision.
///
/// `TIMESTAMPTZ` holds microseconds; `a_clock_reading()` holds nanoseconds. A test
/// that derives a deadline in Rust from a nanosecond reading and compares it
/// against the same instant read back out of the database is comparing two
/// different precisions, and lands on the wrong side of an exact boundary by
/// under a microsecond. Truncating here makes the boundary assertions below
/// mean exactly what they say.
///
/// Production loses the same sub-microsecond tail, and always in the safe
/// direction: a truncated `expires_at` or `last_seen_at` expires marginally
/// sooner, never later.
fn a_clock_reading() -> DateTime<Utc> {
    Utc::now().trunc_subsecs(6)
}

async fn an_operator(db: &SaltDatabase) -> OperatorId {
    named_operator(db, "alice@example.com", "Alice").await
}

async fn named_operator(db: &SaltDatabase, email: &str, display_name: &str) -> OperatorId {
    create_operator(db, email, display_name, "correct horse battery staple")
        .await
        .unwrap()
}

#[sqlx::test]
async fn a_session_is_created_and_loaded_by_its_own_token(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let operator_id = an_operator(&db).await;
    let now = a_clock_reading();

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

    let loaded = load_session(&db, "not-a-real-token", a_clock_reading())
        .await
        .unwrap();

    assert_eq!(loaded, None);
}

#[sqlx::test]
async fn a_session_past_the_absolute_timer_does_not_load_even_when_recently_seen(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let operator_id = an_operator(&db).await;
    let created_at = a_clock_reading();
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
    let created_at = a_clock_reading();
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
    let created_at = a_clock_reading();
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
    let created_at = a_clock_reading();
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
    let now = a_clock_reading();
    let created = create_session(&db, &operator_id, now).await.unwrap();

    delete_session(&db, &created.id).await.unwrap();

    let loaded = load_session(&db, &created.token, now).await.unwrap();
    assert_eq!(loaded, None);
}

#[sqlx::test]
async fn deleting_an_already_deleted_session_is_not_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let operator_id = an_operator(&db).await;
    let created = create_session(&db, &operator_id, a_clock_reading())
        .await
        .unwrap();

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

    let result = create_session(&db, &operator_id, a_clock_reading()).await;

    assert!(result.is_ok());
}

#[sqlx::test]
async fn login_session_creation_refuses_a_disabled_operator(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let operator_id = an_operator(&db).await;
    payroll_app::disable_operator(&db, &operator_id)
        .await
        .unwrap();

    let result = create_session_for_active_operator(&db, &operator_id, a_clock_reading())
        .await
        .unwrap();

    assert_eq!(result, None);
}

// --- Timer boundaries -------------------------------------------------
//
// Each timer is stated as a strict inequality — a session loads *while*
// `now < created_at + 12h` and `now < last_seen_at + 8h`. The tests above
// prove each timer refuses a second past its deadline; these pin the
// deadline itself, so an off-by-one that moved either boundary by a whole
// second in either direction fails here rather than shipping.

#[sqlx::test]
async fn a_session_loads_one_second_before_the_absolute_timer(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let operator_id = an_operator(&db).await;
    let created_at = a_clock_reading();
    let created = create_session(&db, &operator_id, created_at).await.unwrap();

    // Seen at the sixth hour — inside the eight-hour idle window, so this
    // read renews it out to the fourteenth. That leaves the absolute timer
    // as the only one either assertion below can be about.
    load_session(&db, &created.token, created_at + Duration::hours(6))
        .await
        .unwrap();

    let now = created_at + Duration::hours(12) - Duration::seconds(1);

    assert!(
        load_session(&db, &created.token, now)
            .await
            .unwrap()
            .is_some(),
        "the last second before the absolute deadline is still inside it"
    );
}

#[sqlx::test]
async fn a_session_does_not_load_exactly_on_the_absolute_timer(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let operator_id = an_operator(&db).await;
    let created_at = a_clock_reading();
    let created = create_session(&db, &operator_id, created_at).await.unwrap();

    // As above: renewed at the sixth hour, so the idle window reaches the
    // fourteenth and only the absolute timer can end this session.
    load_session(&db, &created.token, created_at + Duration::hours(6))
        .await
        .unwrap();

    // Exactly twelve hours: `now < created_at + 12h` is false, so the
    // session is already over rather than in its last instant.
    let now = created_at + Duration::hours(12);

    assert_eq!(load_session(&db, &created.token, now).await.unwrap(), None);
}

#[sqlx::test]
async fn a_session_does_not_load_exactly_on_the_idle_timer(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let operator_id = an_operator(&db).await;
    let created_at = a_clock_reading();
    let created = create_session(&db, &operator_id, created_at).await.unwrap();

    let now = created_at + Duration::hours(8);

    assert_eq!(load_session(&db, &created.token, now).await.unwrap(), None);
}

#[sqlx::test]
async fn last_seen_at_is_not_extended_at_exactly_five_minutes_stale(pool: PgPool) {
    // "More than five minutes stale" — five minutes exactly is not more.
    let db = SaltDatabase::from_pool(pool.clone());
    let operator_id = an_operator(&db).await;
    let created_at = a_clock_reading();
    let created = create_session(&db, &operator_id, created_at).await.unwrap();

    load_session(&db, &created.token, created_at + Duration::minutes(5))
        .await
        .unwrap();

    assert_eq!(
        last_seen_at(&pool, &created.id).await.timestamp(),
        created_at.timestamp()
    );
}

// --- The absolute timer outlives every extension ----------------------

#[sqlx::test]
async fn extending_the_idle_timer_never_moves_the_absolute_one(pool: PgPool) {
    // The two timers are independent: `last_seen_at` advancing is what keeps
    // a working Operator signed in, and `expires_at` never moving is what
    // ends the day anyway. A session used continuously past the idle window
    // must still stop loading at twelve hours.
    let db = SaltDatabase::from_pool(pool.clone());
    let operator_id = an_operator(&db).await;
    let created_at = a_clock_reading();
    let created = create_session(&db, &operator_id, created_at).await.unwrap();

    // Used every hour, so the idle window never once runs out.
    for hour in 1..12 {
        let now = created_at + Duration::hours(hour);
        assert!(
            load_session(&db, &created.token, now)
                .await
                .unwrap()
                .is_some(),
            "hour {hour} is inside both timers and must load"
        );
        assert_eq!(
            last_seen_at(&pool, &created.id).await.timestamp(),
            now.timestamp(),
            "each of those reads was more than five minutes stale"
        );
    }

    let past_the_day = created_at + Duration::hours(12);

    assert_eq!(
        load_session(&db, &created.token, past_the_day)
            .await
            .unwrap(),
        None,
        "twelve hours of use does not buy a thirteenth"
    );
}

#[sqlx::test]
async fn extending_last_seen_at_only_ever_moves_it_forward(pool: PgPool) {
    // Two concurrent requests each read their own clock, and nothing orders
    // their writes. The later-arriving one may hold the earlier time; it must
    // not shorten the idle window the earlier-arriving one extended.
    let db = SaltDatabase::from_pool(pool.clone());
    let operator_id = an_operator(&db).await;
    let created_at = a_clock_reading();
    let created = create_session(&db, &operator_id, created_at).await.unwrap();

    let later = created_at + Duration::minutes(30);
    let earlier = created_at + Duration::minutes(10);

    load_session(&db, &created.token, later).await.unwrap();
    load_session(&db, &created.token, earlier).await.unwrap();

    assert_eq!(
        last_seen_at(&pool, &created.id).await.timestamp(),
        later.timestamp(),
        "the earlier reading must not have pulled last_seen_at backwards"
    );
}

// --- Lazy deletion touches nothing it did not find --------------------

#[sqlx::test]
async fn a_lookup_of_an_unknown_token_deletes_nothing(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let operator_id = an_operator(&db).await;
    let now = a_clock_reading();
    create_session(&db, &operator_id, now).await.unwrap();

    load_session(&db, "not-a-real-token", now).await.unwrap();

    assert_eq!(session_count(&pool).await, 1);
}

#[sqlx::test]
async fn a_lookup_that_deletes_an_expired_session_leaves_live_ones_alone(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let alice = an_operator(&db).await;
    let bob = named_operator(&db, "bob@example.com", "Bob").await;

    let long_ago = a_clock_reading();
    let expired = create_session(&db, &alice, long_ago).await.unwrap();

    let now = long_ago + Duration::hours(12) + Duration::seconds(1);
    let alices_live = create_session(&db, &alice, now).await.unwrap();
    let bobs_live = create_session(&db, &bob, now).await.unwrap();

    load_session(&db, &expired.token, now).await.unwrap();

    assert_eq!(session_count(&pool).await, 2);
    assert!(
        load_session(&db, &alices_live.token, now)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        load_session(&db, &bobs_live.token, now)
            .await
            .unwrap()
            .is_some()
    );
}

// --- One session is one token -----------------------------------------

#[sqlx::test]
async fn one_operator_holds_many_sessions_and_each_loads_only_itself(pool: PgPool) {
    // ADR-0016 allows many concurrent sessions per Operator. Each is its own
    // row under its own token, so signing out of one leaves the others
    // signed in.
    let db = SaltDatabase::from_pool(pool);
    let operator_id = an_operator(&db).await;
    let now = a_clock_reading();

    let first = create_session(&db, &operator_id, now).await.unwrap();
    let second = create_session(&db, &operator_id, now).await.unwrap();

    assert_ne!(
        first.token, second.token,
        "two sessions never share a token"
    );
    assert_ne!(first.id, second.id);

    assert_eq!(
        load_session(&db, &first.token, now)
            .await
            .unwrap()
            .unwrap()
            .id,
        first.id
    );

    delete_session(&db, &first.id).await.unwrap();

    assert_eq!(load_session(&db, &first.token, now).await.unwrap(), None);
    assert_eq!(
        load_session(&db, &second.token, now)
            .await
            .unwrap()
            .unwrap()
            .id,
        second.id,
        "signing out of one session must not sign out of the other"
    );
}

#[sqlx::test]
async fn the_plaintext_token_is_nowhere_in_the_row_it_created(pool: PgPool) {
    // Only a hash is stored, so the token that was handed back must not be
    // findable by searching for it.
    let db = SaltDatabase::from_pool(pool.clone());
    let operator_id = an_operator(&db).await;
    let created = create_session(&db, &operator_id, a_clock_reading())
        .await
        .unwrap();

    let (token_hash,): (String,) =
        sqlx::query_as("SELECT token_hash FROM session WHERE id = $1::uuid")
            .bind(created.id.as_str())
            .fetch_one(&pool)
            .await
            .unwrap();

    assert_ne!(token_hash, created.token);
    assert!(!token_hash.contains(&created.token));
    assert_eq!(token_hash.len(), 64, "a SHA-256 hex digest, not the token");
}

// --- Clearing an Operator's expired sessions ---------------------------

#[sqlx::test]
async fn clearing_expired_sessions_removes_only_this_operators_expired_rows(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let alice = an_operator(&db).await;
    let bob = named_operator(&db, "bob@example.com", "Bob").await;

    let long_ago = a_clock_reading();
    create_session(&db, &alice, long_ago).await.unwrap();
    create_session(&db, &bob, long_ago).await.unwrap();

    let now = long_ago + Duration::hours(12) + Duration::seconds(1);
    let alices_live = create_session(&db, &alice, now).await.unwrap();

    clear_expired_sessions(&db, &alice, now).await.unwrap();

    assert_eq!(
        session_count(&pool).await,
        2,
        "Alice's expired row is gone, leaving her live one and Bob's untouched expired one"
    );
    assert!(
        load_session(&db, &alices_live.token, now)
            .await
            .unwrap()
            .is_some(),
        "a live session for the same Operator must survive"
    );
}

#[sqlx::test]
async fn clearing_expired_sessions_for_an_operator_with_none_is_a_no_op(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let operator_id = an_operator(&db).await;
    let created = create_session(&db, &operator_id, a_clock_reading())
        .await
        .unwrap();

    clear_expired_sessions(&db, &operator_id, a_clock_reading())
        .await
        .unwrap();

    assert!(
        load_session(&db, &created.token, a_clock_reading())
            .await
            .unwrap()
            .is_some()
    );
}

async fn session_count(pool: &PgPool) -> i64 {
    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM session")
        .fetch_one(pool)
        .await
        .unwrap();
    count
}
