//! Proves the use cases issue #41 introduces: `create_operator`,
//! `find_operator_by_email`, `disable_operator` and
//! `verify_operator_credential` — reached through the public API a later
//! ticket's HTTP layer calls, not raw SQL.

use payroll_app::{
    OperatorStatus, PayrollAppError, SaltDatabase, create_operator, disable_operator,
    find_operator_by_email, verify_operator_credential,
};
use sqlx::PgPool;

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

    let verified =
        verify_operator_credential(&db, "alice@example.com", "correct horse battery staple")
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

    let result = verify_operator_credential(&db, "alice@example.com", "wrong password").await;

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

    let result =
        verify_operator_credential(&db, "alice@example.com", "correct horse battery staple").await;

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

    let unknown_email = verify_operator_credential(&db, "nobody@example.com", "anything").await;
    let wrong_password =
        verify_operator_credential(&db, "alice@example.com", "wrong password").await;
    let disabled =
        verify_operator_credential(&db, "alice@example.com", "correct horse battery staple").await;

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
