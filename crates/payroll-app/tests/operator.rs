//! Proves the use cases issue #41 introduces: `create_operator`,
//! `find_operator_by_email`, `disable_operator` and
//! `verify_operator_credential` — reached through the public API a later
//! ticket's HTTP layer calls, not raw SQL.

use chrono::Utc;
use payroll_app::{
    OperatorStatus, PayrollAppError, SaltDatabase, create_operator, disable_operator,
    find_operator_by_email, verify_operator_credential,
};
use sqlx::{Acquire, PgPool};

#[sqlx::test]
async fn an_operator_is_created_and_found_by_its_own_email(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    create_operator(
        &db,
        "alice@example.com",
        "Alice",
        "correct horse battery staple",
    )
    .await
    .unwrap();

    let found = find_operator_by_email(&db, "alice@example.com")
        .await
        .unwrap()
        .expect("the Operator just created must be found by its own email");

    assert_eq!(found.email, "alice@example.com");
    assert_eq!(found.display_name, "Alice");
    assert_eq!(found.status, OperatorStatus::Active);
}

#[sqlx::test]
async fn finding_an_unknown_email_returns_none(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);

    let found = find_operator_by_email(&db, "nobody@example.com")
        .await
        .unwrap();

    assert_eq!(found, None);
}

#[sqlx::test]
async fn creating_an_operator_with_a_blank_email_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);

    for blank in ["", " ", "\t\n  "] {
        let result = create_operator(&db, blank, "Alice", "a password").await;

        assert_eq!(
            result,
            Err(PayrollAppError::OperatorEmailCannotBeEmpty),
            "an email of {blank:?} must be refused"
        );
    }
}

#[sqlx::test]
async fn creating_an_operator_with_a_blank_display_name_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);

    for blank in ["", " ", "\t\n  "] {
        let result = create_operator(&db, "alice@example.com", blank, "a password").await;

        assert_eq!(
            result,
            Err(PayrollAppError::OperatorDisplayNameCannotBeEmpty),
            "a display name of {blank:?} must be refused"
        );
    }
}

#[sqlx::test]
async fn creating_an_operator_with_a_password_outside_the_length_limits_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);

    let empty = create_operator(&db, "empty@example.com", "Empty", "").await;
    assert_eq!(
        empty,
        Err(PayrollAppError::OperatorPasswordTooShort { minimum: 8 })
    );

    let too_short = create_operator(&db, "short@example.com", "Short", "1234567").await;
    assert_eq!(
        too_short,
        Err(PayrollAppError::OperatorPasswordTooShort { minimum: 8 })
    );

    let too_long_password = "p".repeat(1025);
    let too_long = create_operator(&db, "long@example.com", "Long", &too_long_password).await;
    assert_eq!(
        too_long,
        Err(PayrollAppError::OperatorPasswordTooLong { maximum: 1024 })
    );
}

#[sqlx::test]
async fn email_is_stored_verbatim_but_unique_only_case_insensitively(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    create_operator(&db, "Alice@Example.com", "Alice", "a password")
        .await
        .unwrap();

    // A second Operator whose email folds to the same lower-cased form
    // collides, even though the text differs from the first.
    let result = create_operator(&db, "alice@example.com", "Alice Two", "another password").await;
    assert_eq!(result, Err(PayrollAppError::OperatorEmailAlreadyInUse));

    // The first Operator's own capitalisation survives, and is what a
    // differently-cased lookup finds.
    let found = find_operator_by_email(&db, "ALICE@EXAMPLE.COM")
        .await
        .unwrap()
        .expect("a case-insensitive lookup must still find the Operator");
    assert_eq!(found.email, "Alice@Example.com");
}

#[sqlx::test]
async fn a_correct_credential_verifies(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let created = create_operator(
        &db,
        "alice@example.com",
        "Alice",
        "correct horse battery staple",
    )
    .await
    .unwrap();

    let verified = verify_operator_credential(
        &db,
        "alice@example.com",
        "correct horse battery staple",
        Utc::now(),
    )
    .await
    .unwrap();

    assert_eq!(verified, created);
}

#[sqlx::test]
async fn a_wrong_password_does_not_verify(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    create_operator(
        &db,
        "alice@example.com",
        "Alice",
        "correct horse battery staple",
    )
    .await
    .unwrap();

    let result =
        verify_operator_credential(&db, "alice@example.com", "wrong password", Utc::now()).await;

    assert_eq!(result, Err(PayrollAppError::OperatorCredentialInvalid));
}

#[sqlx::test]
async fn a_disabled_operators_credential_does_not_verify(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let operator_id = create_operator(
        &db,
        "alice@example.com",
        "Alice",
        "correct horse battery staple",
    )
    .await
    .unwrap();
    disable_operator(&db, &operator_id).await.unwrap();

    let result = verify_operator_credential(
        &db,
        "alice@example.com",
        "correct horse battery staple",
        Utc::now(),
    )
    .await;

    assert_eq!(result, Err(PayrollAppError::OperatorCredentialInvalid));
}

/// The refusal for an unknown email, a wrong password and a disabled
/// Operator is the exact same value — not merely three errors that print the
/// same message, but one enum variant carrying no data — so nothing a caller
/// can inspect tells the three apart (issue #41's own acceptance criterion).
#[sqlx::test]
async fn an_unknown_email_a_wrong_password_and_a_disabled_operator_refuse_identically(
    pool: PgPool,
) {
    let db = SaltDatabase::from_pool(pool);
    let operator_id = create_operator(
        &db,
        "alice@example.com",
        "Alice",
        "correct horse battery staple",
    )
    .await
    .unwrap();
    disable_operator(&db, &operator_id).await.unwrap();

    let now = Utc::now();
    let unknown_email =
        verify_operator_credential(&db, "nobody@example.com", "anything", now).await;
    let wrong_password =
        verify_operator_credential(&db, "alice@example.com", "wrong password", now).await;
    let disabled = verify_operator_credential(
        &db,
        "alice@example.com",
        "correct horse battery staple",
        now,
    )
    .await;

    assert_eq!(
        unknown_email,
        Err(PayrollAppError::OperatorCredentialInvalid)
    );
    assert_eq!(
        wrong_password,
        Err(PayrollAppError::OperatorCredentialInvalid)
    );
    assert_eq!(disabled, Err(PayrollAppError::OperatorCredentialInvalid));
}

#[sqlx::test]
async fn the_password_is_not_recoverable_from_the_stored_verifier(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let password = "correct horse battery staple";
    create_operator(&db, "alice@example.com", "Alice", password)
        .await
        .unwrap();

    let stored: String = sqlx::query_scalar("SELECT password_verifier FROM operator")
        .fetch_one(&pool)
        .await
        .unwrap();

    assert!(
        stored.starts_with("$argon2id$"),
        "expected an Argon2id PHC string, got {stored:?}"
    );
    assert!(
        !stored.contains(password),
        "the stored verifier must never contain the plaintext password"
    );
}

#[sqlx::test]
async fn disabling_an_operator_is_reflected_by_a_later_lookup(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let operator_id = create_operator(&db, "alice@example.com", "Alice", "a password")
        .await
        .unwrap();

    disable_operator(&db, &operator_id).await.unwrap();

    let found = find_operator_by_email(&db, "alice@example.com")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.status, OperatorStatus::Disabled);
}

#[sqlx::test]
async fn disabling_an_already_disabled_operator_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let operator_id = create_operator(&db, "alice@example.com", "Alice", "a password")
        .await
        .unwrap();
    disable_operator(&db, &operator_id).await.unwrap();

    let result = disable_operator(&db, &operator_id).await;

    assert_eq!(
        result,
        Err(PayrollAppError::OperatorAlreadyDisabled(operator_id))
    );
}

/// Folding case does not make ` alice@x` collide with `alice@x`, so without
/// trimming, Salt would hold two Operators that every human reads as one and
/// a sign-in would land on whichever the typist happened to reproduce. The
/// address's own capitalisation still survives; only the whitespace around it
/// does not.
#[sqlx::test]
async fn an_email_padded_with_whitespace_is_the_same_email(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let created = create_operator(&db, "  Alice@Example.com\t", "Alice", "a password")
        .await
        .unwrap();

    let collision = create_operator(&db, "alice@example.com", "Alice Two", "a password").await;
    assert_eq!(collision, Err(PayrollAppError::OperatorEmailAlreadyInUse));

    let found = find_operator_by_email(&db, "\n alice@example.com ")
        .await
        .unwrap()
        .expect("a padded lookup must find the Operator");
    assert_eq!(found.id, created);
    assert_eq!(
        found.email, "Alice@Example.com",
        "the stored email keeps its capitalisation and loses only the padding"
    );

    let verified = verify_operator_credential(&db, " alice@example.com ", "a password", Utc::now())
        .await
        .unwrap();
    assert_eq!(verified, created);
}

/// A display name is trimmed for the same reason, so the name Salt shows a
/// person is not silently indented by whatever a form submitted.
#[sqlx::test]
async fn a_display_name_padded_with_whitespace_is_stored_trimmed(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    create_operator(&db, "alice@example.com", "  Alice  ", "a password")
        .await
        .unwrap();

    let found = find_operator_by_email(&db, "alice@example.com")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.display_name, "Alice");
}

/// `verify_operator_credential` is reachable by anyone who can reach a sign-in
/// route, so the work one attempt costs must not be the caller's to choose. A
/// password longer than `create_operator` would ever have recorded is refused
/// with the same opaque refusal as every other failure — it says nothing about
/// any account — rather than being hashed.
#[sqlx::test]
async fn verifying_an_over_long_password_is_refused_without_hashing_it(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    create_operator(&db, "alice@example.com", "Alice", "a password")
        .await
        .unwrap();

    let over_long = "p".repeat(1025);

    let now = Utc::now();
    let known = verify_operator_credential(&db, "alice@example.com", &over_long, now).await;
    let unknown = verify_operator_credential(&db, "nobody@example.com", &over_long, now).await;

    assert_eq!(known, Err(PayrollAppError::OperatorCredentialInvalid));
    assert_eq!(unknown, Err(PayrollAppError::OperatorCredentialInvalid));
}

/// Issue #41's timing criterion, asserted as work rather than as a source
/// comment: an unknown email must still pay the Argon2id cost a real row
/// costs, so response time does not separate "no such account" from "wrong
/// password".
///
/// Only a *lower* bound is asserted, and a generous one. A loaded machine
/// makes both measurements slower, never the unknown-email one faster, so the
/// direction that could fail spuriously is not the direction being asserted.
/// A short-circuit on "no row" — the mistake this guards — removes an entire
/// Argon2id hash and lands orders of magnitude below the bound, not near it.
#[sqlx::test]
async fn verifying_an_unknown_email_still_pays_the_argon2id_cost(pool: PgPool) {
    use std::time::Instant;

    let db = SaltDatabase::from_pool(pool);
    create_operator(&db, "alice@example.com", "Alice", "a password")
        .await
        .unwrap();

    async fn fastest_of_three(db: &SaltDatabase, email: &str) -> std::time::Duration {
        let mut fastest = std::time::Duration::MAX;
        for _ in 0..3 {
            let started = Instant::now();
            let refused =
                verify_operator_credential(db, email, "the wrong password", Utc::now()).await;
            assert_eq!(refused, Err(PayrollAppError::OperatorCredentialInvalid));
            fastest = fastest.min(started.elapsed());
        }
        fastest
    }

    // The fastest run of each, so a scheduling stall inflates neither side.
    let wrong_password = fastest_of_three(&db, "alice@example.com").await;
    let unknown_email = fastest_of_three(&db, "nobody@example.com").await;

    assert!(
        unknown_email * 2 >= wrong_password,
        "an unknown email took {unknown_email:?} against {wrong_password:?} for a wrong \
         password, so it is not doing the same Argon2id work"
    );
}

/// Issue #42's own acceptance criterion: nine wrong passwords still let the
/// tenth attempt so much as check the password, and a correct one on the
/// tenth still verifies. Ten is the first count that locks, not the last
/// that is allowed through.
#[sqlx::test]
async fn nine_failures_do_not_lock_the_account(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let created = create_operator(
        &db,
        "alice@example.com",
        "Alice",
        "correct horse battery staple",
    )
    .await
    .unwrap();

    let now = Utc::now();
    for _ in 0..9 {
        let result =
            verify_operator_credential(&db, "alice@example.com", "wrong password", now).await;
        assert_eq!(result, Err(PayrollAppError::OperatorCredentialInvalid));
    }

    let verified = verify_operator_credential(
        &db,
        "alice@example.com",
        "correct horse battery staple",
        now,
    )
    .await
    .unwrap();
    assert_eq!(verified, created);
}

/// Ten failures inside the fifteen-minute window locks the account, and the
/// lock refuses even the correct password (issue #42's acceptance criteria).
#[sqlx::test]
async fn ten_failures_inside_the_window_lock_the_account_even_against_the_right_password(
    pool: PgPool,
) {
    let db = SaltDatabase::from_pool(pool);
    create_operator(
        &db,
        "alice@example.com",
        "Alice",
        "correct horse battery staple",
    )
    .await
    .unwrap();

    let now = Utc::now();
    for _ in 0..10 {
        let result =
            verify_operator_credential(&db, "alice@example.com", "wrong password", now).await;
        assert_eq!(result, Err(PayrollAppError::OperatorCredentialInvalid));
    }

    let result = verify_operator_credential(
        &db,
        "alice@example.com",
        "correct horse battery staple",
        now,
    )
    .await;
    assert_eq!(result, Err(PayrollAppError::OperatorCredentialInvalid));
}

/// The lock lifts by itself fifteen minutes later, with no administrator in
/// the loop (issue #42's own acceptance criterion) — proven by passing a
/// later `now` rather than sleeping on the wall clock.
#[sqlx::test]
async fn the_lock_lifts_by_itself_fifteen_minutes_later(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let created = create_operator(
        &db,
        "alice@example.com",
        "Alice",
        "correct horse battery staple",
    )
    .await
    .unwrap();

    let now = Utc::now();
    for _ in 0..10 {
        let result =
            verify_operator_credential(&db, "alice@example.com", "wrong password", now).await;
        assert_eq!(result, Err(PayrollAppError::OperatorCredentialInvalid));
    }

    let still_locked = verify_operator_credential(
        &db,
        "alice@example.com",
        "correct horse battery staple",
        now + chrono::Duration::minutes(14),
    )
    .await;
    assert_eq!(
        still_locked,
        Err(PayrollAppError::OperatorCredentialInvalid)
    );

    // A locked wrong password receives the same refusal but must not restart
    // the window. It is still the window opened by the first failure that
    // ends at the fifteen-minute boundary below.
    let locked_wrong_password = verify_operator_credential(
        &db,
        "alice@example.com",
        "wrong password",
        now + chrono::Duration::minutes(14),
    )
    .await;
    assert_eq!(
        locked_wrong_password,
        Err(PayrollAppError::OperatorCredentialInvalid)
    );

    let unlocked = verify_operator_credential(
        &db,
        "alice@example.com",
        "correct horse battery staple",
        now + chrono::Duration::minutes(15),
    )
    .await
    .unwrap();
    assert_eq!(unlocked, created);
}

/// The decision to authenticate and the counter transition are one operation:
/// a correct attempt that waits behind a concurrent tenth failure must inspect
/// the committed lock and refuse. This holds the tenth failure uncommitted so
/// the test distinguishes a locking read from a stale ordinary `SELECT`.
#[sqlx::test]
async fn a_correct_credential_waiting_behind_the_tenth_failure_is_refused(pool: PgPool) {
    use tokio::sync::oneshot;

    let db = SaltDatabase::from_pool(pool.clone());
    let operator_id = create_operator(
        &db,
        "alice@example.com",
        "Alice",
        "correct horse battery staple",
    )
    .await
    .unwrap();

    let now = Utc::now();
    for _ in 0..9 {
        let result =
            verify_operator_credential(&db, "alice@example.com", "wrong password", now).await;
        assert_eq!(result, Err(PayrollAppError::OperatorCredentialInvalid));
    }

    // This direct SQL is test control rather than use-case setup: only an
    // uncommitted writer can put the credential check on the dangerous side
    // of the read/write race that the row lock must close.
    let mut holder = pool.acquire().await.unwrap();
    let mut holder_tx = holder.begin().await.unwrap();
    sqlx::query("UPDATE operator SET failed_attempt_count = 10 WHERE id = $1::uuid")
        .bind(operator_id.as_str())
        .execute(&mut *holder_tx)
        .await
        .unwrap();

    let (started_sender, started_receiver) = oneshot::channel();
    let racing_db = SaltDatabase::from_pool(pool);
    let credential_check = tokio::spawn(async move {
        started_sender.send(()).unwrap();
        verify_operator_credential(
            &racing_db,
            "alice@example.com",
            "correct horse battery staple",
            now,
        )
        .await
    });

    started_receiver.await.unwrap();
    // Without `FOR UPDATE`, the check reads the pre-lock count, completes
    // Argon2id, then waits only when it tries to clear the counter below.
    // With the lock it is already waiting at the read.
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    holder_tx.commit().await.unwrap();

    assert_eq!(
        credential_check.await.unwrap(),
        Err(PayrollAppError::OperatorCredentialInvalid)
    );
}

/// A successful login resets the counter to zero (issue #42's own acceptance
/// criterion): nine failures followed by a success must not leave the tenth
/// wrong password after it locking the account.
#[sqlx::test]
async fn a_successful_login_resets_the_failure_counter(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let created = create_operator(
        &db,
        "alice@example.com",
        "Alice",
        "correct horse battery staple",
    )
    .await
    .unwrap();

    let now = Utc::now();
    for _ in 0..9 {
        let result =
            verify_operator_credential(&db, "alice@example.com", "wrong password", now).await;
        assert_eq!(result, Err(PayrollAppError::OperatorCredentialInvalid));
    }

    verify_operator_credential(
        &db,
        "alice@example.com",
        "correct horse battery staple",
        now,
    )
    .await
    .unwrap();

    // The counter is zero again, so one more wrong password is only the
    // first failure of a new window, not the tenth of the old one.
    let result = verify_operator_credential(&db, "alice@example.com", "wrong password", now).await;
    assert_eq!(result, Err(PayrollAppError::OperatorCredentialInvalid));

    let verified = verify_operator_credential(
        &db,
        "alice@example.com",
        "correct horse battery staple",
        now,
    )
    .await
    .unwrap();
    assert_eq!(verified, created);
}

/// A locked account's refusal is indistinguishable from a wrong password
/// (issue #42's own acceptance criterion): the same enum variant, carrying
/// nothing that would let a caller tell "locked" apart from "wrong
/// password" or "no such account".
#[sqlx::test]
async fn a_locked_account_refuses_identically_to_a_wrong_password(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    create_operator(
        &db,
        "alice@example.com",
        "Alice",
        "correct horse battery staple",
    )
    .await
    .unwrap();

    let now = Utc::now();
    for _ in 0..10 {
        verify_operator_credential(&db, "alice@example.com", "wrong password", now)
            .await
            .unwrap_err();
    }

    let locked = verify_operator_credential(
        &db,
        "alice@example.com",
        "correct horse battery staple",
        now,
    )
    .await;
    let wrong_password =
        verify_operator_credential(&db, "alice@example.com", "wrong password", now).await;
    let unknown_email =
        verify_operator_credential(&db, "nobody@example.com", "anything", now).await;

    assert_eq!(locked, Err(PayrollAppError::OperatorCredentialInvalid));
    assert_eq!(
        wrong_password,
        Err(PayrollAppError::OperatorCredentialInvalid)
    );
    assert_eq!(
        unknown_email,
        Err(PayrollAppError::OperatorCredentialInvalid)
    );
}
