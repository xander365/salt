//! `PersonParticulars` and Person `full_name` correction (issue #72, parent
//! #70 D-7; CONTEXT.md). A Person's identity number and address — one row
//! per Person, correctable forever with a stated reason — plus the ability
//! to fix a misspelled `full_name` after the fact, which migration 0031 made
//! append-only in anticipation of exactly this ticket.
//!
//! `PersonParticulars` are not Owner-only (D25 restricts only Employer
//! particulars): any active member of the Employer may record or correct
//! either fact here, so unlike `employer_particulars.rs` there is no
//! `require_role` for `salt-server`'s handlers to call.
//!
//! Both write functions take `employer_id` explicitly and check the named
//! `person_id` against it inside their own query, refusing a `person_id`
//! belonging to another Employer exactly like one that names nothing at all
//! ([`PayrollAppError::PersonNotFound`], ADR-0017) — the same check
//! `create_employment`'s `EmploymentPerson::Existing` branch makes.
//!
//! Nothing here freezes into a `FinalizedPayroll` — that is #70 D-7's own
//! later ticket — so `person_particulars` and `person.full_name` are read
//! live by whatever renders a payslip until then. The divergence
//! acknowledgement below exists to keep the audit trail honest in the
//! meantime, not because anything downstream depends on it yet.

use chrono::{DateTime, Utc};
use payroll::{EmployerId, PayPeriod, PersonId};

use crate::action_log::{ActionLogEntry, ActionType, write_action_log_entry};
use crate::database::SaltDatabase;
use crate::error::PayrollAppError;
use crate::freeze::{
    diverging_periods_json, live_finalized_periods_for_person, require_acknowledgement_of_person,
};

/// One Person's particulars screen: the `full_name` every Employment already
/// shows, plus whatever `person_particulars` this Person has on record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonParticulars {
    pub full_name: String,
    pub identity_number: Option<String>,
    pub address_line1: Option<String>,
    pub address_line2: Option<String>,
    pub city: Option<String>,
    pub postal_code: Option<String>,
    pub particulars_created_at: Option<DateTime<Utc>>,
    pub particulars_created_by: Option<String>,
}

/// The fields [`set_person_particulars`] writes, named once so the use case
/// and its ActionLog "before"/"after" JSON build from the same shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonParticularsFields {
    pub identity_number: String,
    pub address_line1: String,
    pub address_line2: Option<String>,
    pub city: String,
    pub postal_code: Option<String>,
}

fn fields_json(fields: &PersonParticularsFields) -> serde_json::Value {
    serde_json::json!({
        "identity_number": fields.identity_number,
        "address_line1": fields.address_line1,
        "address_line2": fields.address_line2,
        "city": fields.city,
        "postal_code": fields.postal_code,
    })
}

/// The five columns [`set_person_particulars`] reads back before it
/// overwrites them, converted straight into [`PersonParticularsFields`] so
/// the ActionLog's "before" is built by the same [`fields_json`] that builds
/// its "after".
type StoredFields = (String, String, Option<String>, String, Option<String>);

impl From<StoredFields> for PersonParticularsFields {
    fn from(stored: StoredFields) -> Self {
        let (identity_number, address_line1, address_line2, city, postal_code) = stored;
        Self {
            identity_number,
            address_line1,
            address_line2,
            city,
            postal_code,
        }
    }
}

/// Stores every particulars value trimmed, while treating a whitespace-only
/// optional value as absent. This crate owns the invariant at the use-case
/// boundary (ADR-0018), before either persistence or ActionLog serialization.
fn normalize_fields(fields: PersonParticularsFields) -> PersonParticularsFields {
    fn stated(value: Option<String>) -> Option<String> {
        value.and_then(|value| {
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_string())
        })
    }

    PersonParticularsFields {
        identity_number: fields.identity_number.trim().to_string(),
        address_line1: fields.address_line1.trim().to_string(),
        address_line2: stated(fields.address_line2),
        city: fields.city.trim().to_string(),
        postal_code: stated(fields.postal_code),
    }
}

/// Confirms `person_id` belongs to `employer_id`, returning its current
/// `full_name` — refusing exactly like a missing Person (ADR-0017) when it
/// does not, the same check [`crate::create_employment`]'s
/// `EmploymentPerson::Existing` branch makes.
async fn person_full_name(
    conn: &mut sqlx::PgConnection,
    employer_id: &EmployerId,
    person_id: &PersonId,
    for_update: bool,
) -> Result<String, PayrollAppError> {
    let query = if for_update {
        "SELECT full_name FROM person WHERE id = $1 AND employer_id = $2 FOR UPDATE"
    } else {
        "SELECT full_name FROM person WHERE id = $1 AND employer_id = $2"
    };
    let full_name: Option<String> = sqlx::query_scalar(query)
        .bind(person_id.as_str())
        .bind(employer_id.as_str())
        .fetch_optional(conn)
        .await?;
    full_name.ok_or_else(|| PayrollAppError::PersonNotFound(person_id.clone()))
}

/// The frozen snapshot [`crate::finalize::finalize_payroll_run`] writes onto
/// one member's `FinalizedPayroll` for `person_id` (issue #73, CONTEXT.md's
/// own glossary entry: "the Person's full name, identity number and
/// address"). Unlike
/// [`crate::employer_particulars::employer_particulars_snapshot`], this
/// never has nothing to freeze: `full_name` is set the moment
/// `create_employment` creates the Person and is never absent, whatever
/// `person_particulars` this Person has, or has not, recorded.
///
/// Takes a plain connection and locks nothing itself, for the same reason
/// `employer_particulars_snapshot` does: `finalize_payroll_run` calls this
/// only after locking every member's `employment` row, and that lock is
/// exactly what [`set_person_particulars`] and [`correct_person_full_name`]
/// take `FOR UPDATE` on (via `lock_this_persons_employments`) before either
/// writes this Person's facts (`freeze.rs`'s module doc).
pub(crate) async fn person_particulars_snapshot(
    conn: &mut sqlx::PgConnection,
    person_id: &PersonId,
) -> Result<serde_json::Value, PayrollAppError> {
    type Row = (
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let row: Option<Row> = sqlx::query_as(
        "SELECT person.full_name, particulars.identity_number, particulars.address_line1,
                particulars.address_line2, particulars.city, particulars.postal_code
         FROM person
         LEFT JOIN person_particulars AS particulars ON particulars.person_id = person.id
         WHERE person.id = $1",
    )
    .bind(person_id.as_str())
    .fetch_optional(conn)
    .await?;

    let (full_name, identity_number, address_line1, address_line2, city, postal_code) = row.expect(
        "employment.person_id always names a Person, and person is never deleted \
             (migration 0031)",
    );

    Ok(serde_json::json!({
        "full_name": full_name,
        "identity_number": identity_number,
        "address_line1": address_line1,
        "address_line2": address_line2,
        "city": city,
        "postal_code": postal_code,
    }))
}

/// `GET`'s own read: the Person's `full_name`, plus `None` particulars
/// fields when this Person has never recorded them.
pub async fn get_person_particulars(
    db: &SaltDatabase,
    employer_id: &EmployerId,
    person_id: &PersonId,
) -> Result<PersonParticulars, PayrollAppError> {
    type Row = (
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<DateTime<Utc>>,
        Option<String>,
    );
    let row: Option<Row> = sqlx::query_as(
        "SELECT person.full_name, particulars.identity_number, particulars.address_line1,
                particulars.address_line2, particulars.city, particulars.postal_code,
                particulars.created_at, particulars.created_by
         FROM person
         LEFT JOIN person_particulars AS particulars ON particulars.person_id = person.id
         WHERE person.id = $1 AND person.employer_id = $2",
    )
    .bind(person_id.as_str())
    .bind(employer_id.as_str())
    .fetch_optional(db.pool())
    .await?;

    let (
        full_name,
        identity_number,
        address_line1,
        address_line2,
        city,
        postal_code,
        particulars_created_at,
        particulars_created_by,
    ) = row.ok_or_else(|| PayrollAppError::PersonNotFound(person_id.clone()))?;

    Ok(PersonParticulars {
        full_name,
        identity_number,
        address_line1,
        address_line2,
        city,
        postal_code,
        particulars_created_at,
        particulars_created_by,
    })
}

/// Records or corrects `person_id`'s `PersonParticulars` — an insert when
/// none exists yet, an update when one does, the same singleton-fact pattern
/// [`crate::set_employer_particulars`] uses (that function's own docs
/// explain the insert-vs-correct decision in full).
///
/// Locks every Employment row this Person has, in `id` order, before reading
/// the divergence list: `finalize_payroll_run` takes its own `FOR SHARE` on
/// each member's `employment` row in that same order
/// (`finalize.rs::lock_member_employments`), so a finalization of any of
/// this Person's Employments and a correction here can never interleave.
pub async fn set_person_particulars(
    db: &SaltDatabase,
    employer_id: &EmployerId,
    person_id: &PersonId,
    fields: PersonParticularsFields,
    acknowledged_diverging_periods: &[PayPeriod],
    reason: &str,
    actor: &str,
) -> Result<Vec<PayPeriod>, PayrollAppError> {
    let fields = normalize_fields(fields);

    if fields.identity_number.trim().is_empty() {
        return Err(PayrollAppError::PersonParticularsIdentityNumberCannotBeEmpty);
    }
    if fields.address_line1.trim().is_empty() {
        return Err(PayrollAppError::PersonParticularsAddressLine1CannotBeEmpty);
    }
    if fields.city.trim().is_empty() {
        return Err(PayrollAppError::PersonParticularsCityCannotBeEmpty);
    }

    let mut tx = db.pool().begin().await?;

    person_full_name(&mut tx, employer_id, person_id, false).await?;
    lock_this_persons_employments(&mut tx, person_id).await?;

    let existing: Option<StoredFields> = sqlx::query_as(
        "SELECT identity_number, address_line1, address_line2, city, postal_code
         FROM person_particulars
         WHERE person_id = $1
         FOR UPDATE",
    )
    .bind(person_id.as_str())
    .fetch_optional(&mut *tx)
    .await?;
    let is_correction = existing.is_some();

    if is_correction && reason.trim().is_empty() {
        return Err(PayrollAppError::PersonParticularsCorrectionReasonCannotBeEmpty);
    }

    let diverging_periods = live_finalized_periods_for_person(&mut tx, person_id).await?;
    require_acknowledgement_of_person(
        person_id,
        &diverging_periods,
        acknowledged_diverging_periods,
    )?;

    let diverges = !diverging_periods.is_empty();
    if !is_correction && diverges && reason.trim().is_empty() {
        return Err(PayrollAppError::PersonParticularsCorrectionReasonCannotBeEmpty);
    }

    if is_correction {
        sqlx::query(
            "UPDATE person_particulars
             SET identity_number = $2, address_line1 = $3, address_line2 = $4, city = $5,
                 postal_code = $6
             WHERE person_id = $1",
        )
        .bind(person_id.as_str())
        .bind(&fields.identity_number)
        .bind(&fields.address_line1)
        .bind(&fields.address_line2)
        .bind(&fields.city)
        .bind(&fields.postal_code)
        .execute(&mut *tx)
        .await?;
    } else {
        sqlx::query(
            "INSERT INTO person_particulars
                (person_id, identity_number, address_line1, address_line2, city, postal_code,
                 created_by)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(person_id.as_str())
        .bind(&fields.identity_number)
        .bind(&fields.address_line1)
        .bind(&fields.address_line2)
        .bind(&fields.city)
        .bind(&fields.postal_code)
        .bind(actor)
        .execute(&mut *tx)
        .await?;
    }

    if is_correction || diverges {
        write_action_log_entry(
            &mut tx,
            ActionLogEntry {
                employer_id,
                actor,
                action_type: ActionType::PersonParticularsCorrected,
                target_type: "person",
                target_id: person_id.as_str(),
                context: Some(serde_json::json!({
                    "reason": reason,
                    "before": existing
                        .map(PersonParticularsFields::from)
                        .as_ref()
                        .map(fields_json),
                    "after": fields_json(&fields),
                    "diverging_live_finalized_periods": diverging_periods_json(&diverging_periods),
                })),
            },
        )
        .await?;
    }

    tx.commit().await?;
    Ok(diverging_periods)
}

/// Corrects a misspelled `full_name` (issue #72's own headline acceptance
/// criterion). Unlike [`set_person_particulars`], there is no "first record"
/// case — `full_name` is set the moment `create_employment` creates the
/// Person, so `reason` is demanded unconditionally.
pub async fn correct_person_full_name(
    db: &SaltDatabase,
    employer_id: &EmployerId,
    person_id: &PersonId,
    full_name: &str,
    acknowledged_diverging_periods: &[PayPeriod],
    reason: &str,
    actor: &str,
) -> Result<Vec<PayPeriod>, PayrollAppError> {
    // Stored trimmed, not merely checked for content, for the same reason
    // `create_employment` trims a brand-new Person's name (issue #72's own
    // Deep Instructions): surrounding whitespace a form submitted would
    // otherwise be unfixable by this very correction path.
    let full_name = full_name.trim();
    if full_name.is_empty() {
        return Err(PayrollAppError::PersonFullNameCannotBeEmpty);
    }
    if reason.trim().is_empty() {
        return Err(PayrollAppError::PersonNameCorrectionReasonCannotBeEmpty);
    }

    let mut tx = db.pool().begin().await?;

    let before = person_full_name(&mut tx, employer_id, person_id, true).await?;
    lock_this_persons_employments(&mut tx, person_id).await?;

    let diverging_periods = live_finalized_periods_for_person(&mut tx, person_id).await?;
    require_acknowledgement_of_person(
        person_id,
        &diverging_periods,
        acknowledged_diverging_periods,
    )?;

    sqlx::query("UPDATE person SET full_name = $2 WHERE id = $1")
        .bind(person_id.as_str())
        .bind(full_name)
        .execute(&mut *tx)
        .await?;

    write_action_log_entry(
        &mut tx,
        ActionLogEntry {
            employer_id,
            actor,
            action_type: ActionType::PersonFullNameCorrected,
            target_type: "person",
            target_id: person_id.as_str(),
            context: Some(serde_json::json!({
                "reason": reason,
                "before": { "full_name": before },
                "after": { "full_name": full_name },
                "diverging_live_finalized_periods": diverging_periods_json(&diverging_periods),
            })),
        },
    )
    .await?;

    tx.commit().await?;
    Ok(diverging_periods)
}

/// `FOR UPDATE` on every Employment row `person_id` has, in `id` order —
/// this is what makes the divergence list both functions above read hold
/// against a concurrent `finalize_payroll_run`, which locks each member's
/// `employment` row (also in `id` order) before it reads any master data
/// (`finalize.rs`'s own `lock_member_employments`). The shared order rules
/// out a deadlock between the two: whichever arrives second always waits on
/// a lock the first already holds, rather than each waiting on the other.
async fn lock_this_persons_employments(
    conn: &mut sqlx::PgConnection,
    person_id: &PersonId,
) -> Result<(), PayrollAppError> {
    sqlx::query("SELECT id FROM employment WHERE person_id = $1 ORDER BY id FOR UPDATE")
        .bind(person_id.as_str())
        .fetch_all(conn)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use payroll::{DayOfMonth, PaySchedule, PeriodEndDay};
    use sqlx::PgPool;

    use super::*;
    use crate::employer::create_employer;
    use crate::employment::{EmploymentPerson, create_employment};

    fn fields(identity_number: &str) -> PersonParticularsFields {
        PersonParticularsFields {
            identity_number: identity_number.to_string(),
            address_line1: "1 Independence Ave".to_string(),
            address_line2: None,
            city: "Windhoek".to_string(),
            postal_code: Some("10001".to_string()),
        }
    }

    async fn an_employer(db: &SaltDatabase) -> EmployerId {
        create_employer(
            db,
            "Acme Corp",
            PaySchedule::new(PeriodEndDay::Day(DayOfMonth::new(25).unwrap())),
            "test-setup",
        )
        .await
        .unwrap()
    }

    async fn a_person(db: &SaltDatabase, employer_id: &EmployerId, full_name: &str) -> PersonId {
        let (person_id, _employment_id) = create_employment(
            db,
            employer_id,
            EmploymentPerson::New(full_name.to_string()),
            chrono::NaiveDate::from_ymd_opt(2026, 1, 26).unwrap(),
            None,
            "test-setup",
        )
        .await
        .unwrap();
        person_id
    }

    #[sqlx::test]
    async fn no_particulars_read_back_with_none_fields_but_the_full_name(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool);
        let employer_id = an_employer(&db).await;
        let person_id = a_person(&db, &employer_id, "Ada Lovelace").await;

        let particulars = get_person_particulars(&db, &employer_id, &person_id)
            .await
            .unwrap();
        assert_eq!(particulars.full_name, "Ada Lovelace");
        assert_eq!(particulars.identity_number, None);
        assert_eq!(particulars.particulars_created_at, None);
    }

    #[sqlx::test]
    async fn an_unknown_person_is_refused(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool);
        let employer_id = an_employer(&db).await;

        let err = get_person_particulars(&db, &employer_id, &PersonId::new("no-such-person"))
            .await
            .unwrap_err();

        assert_eq!(
            err,
            PayrollAppError::PersonNotFound(PersonId::new("no-such-person"))
        );
    }

    #[sqlx::test]
    async fn a_person_id_belonging_to_another_employer_is_refused_like_one_that_does_not_exist(
        pool: PgPool,
    ) {
        let db = SaltDatabase::from_pool(pool);
        let employer_id = an_employer(&db).await;
        let other_employer_id = an_employer(&db).await;
        let person_id = a_person(&db, &employer_id, "Ada Lovelace").await;

        let err = get_person_particulars(&db, &other_employer_id, &person_id)
            .await
            .unwrap_err();
        assert_eq!(err, PayrollAppError::PersonNotFound(person_id.clone()));

        let err = set_person_particulars(
            &db,
            &other_employer_id,
            &person_id,
            fields("12345678901"),
            &[],
            "",
            "operator:alice",
        )
        .await
        .unwrap_err();
        assert_eq!(err, PayrollAppError::PersonNotFound(person_id.clone()));

        let err = correct_person_full_name(
            &db,
            &other_employer_id,
            &person_id,
            "New Name",
            &[],
            "reason",
            "operator:alice",
        )
        .await
        .unwrap_err();
        assert_eq!(err, PayrollAppError::PersonNotFound(person_id));
    }

    #[sqlx::test]
    async fn a_first_record_with_nothing_to_diverge_from_needs_no_reason(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool);
        let employer_id = an_employer(&db).await;
        let person_id = a_person(&db, &employer_id, "Ada Lovelace").await;

        let diverging = set_person_particulars(
            &db,
            &employer_id,
            &person_id,
            fields("12345678901"),
            &[],
            "",
            "operator:alice",
        )
        .await
        .unwrap();

        assert_eq!(diverging, Vec::new());
        let stored = get_person_particulars(&db, &employer_id, &person_id)
            .await
            .unwrap();
        assert_eq!(stored.identity_number, Some("12345678901".to_string()));
        assert_eq!(
            stored.particulars_created_by,
            Some("operator:alice".to_string())
        );
    }

    #[sqlx::test]
    async fn particulars_are_stored_trimmed(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool);
        let employer_id = an_employer(&db).await;
        let person_id = a_person(&db, &employer_id, "Ada Lovelace").await;

        set_person_particulars(
            &db,
            &employer_id,
            &person_id,
            PersonParticularsFields {
                identity_number: "  12345678901  ".to_string(),
                address_line1: "  1 Independence Ave  ".to_string(),
                address_line2: Some("  Apartment 2  ".to_string()),
                city: "  Windhoek  ".to_string(),
                postal_code: Some("  10001  ".to_string()),
            },
            &[],
            "",
            "operator:alice",
        )
        .await
        .unwrap();

        let stored = get_person_particulars(&db, &employer_id, &person_id)
            .await
            .unwrap();
        assert_eq!(stored.identity_number.as_deref(), Some("12345678901"));
        assert_eq!(stored.address_line1.as_deref(), Some("1 Independence Ave"));
        assert_eq!(stored.address_line2.as_deref(), Some("Apartment 2"));
        assert_eq!(stored.city.as_deref(), Some("Windhoek"));
        assert_eq!(stored.postal_code.as_deref(), Some("10001"));
    }

    #[sqlx::test]
    async fn a_blank_identity_number_is_refused(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool);
        let employer_id = an_employer(&db).await;
        let person_id = a_person(&db, &employer_id, "Ada Lovelace").await;

        let err = set_person_particulars(
            &db,
            &employer_id,
            &person_id,
            fields("   "),
            &[],
            "",
            "operator:alice",
        )
        .await
        .unwrap_err();

        assert_eq!(
            err,
            PayrollAppError::PersonParticularsIdentityNumberCannotBeEmpty
        );
    }

    #[sqlx::test]
    async fn correcting_an_existing_row_demands_a_reason_even_with_nothing_to_diverge_from(
        pool: PgPool,
    ) {
        let db = SaltDatabase::from_pool(pool);
        let employer_id = an_employer(&db).await;
        let person_id = a_person(&db, &employer_id, "Ada Lovelace").await;
        set_person_particulars(
            &db,
            &employer_id,
            &person_id,
            fields("12345678901"),
            &[],
            "",
            "operator:alice",
        )
        .await
        .unwrap();

        let err = set_person_particulars(
            &db,
            &employer_id,
            &person_id,
            fields("98765432109"),
            &[],
            "",
            "operator:alice",
        )
        .await
        .unwrap_err();

        assert_eq!(
            err,
            PayrollAppError::PersonParticularsCorrectionReasonCannotBeEmpty
        );
        assert_eq!(
            get_person_particulars(&db, &employer_id, &person_id)
                .await
                .unwrap()
                .identity_number,
            Some("12345678901".to_string())
        );
    }

    #[sqlx::test]
    async fn a_reasoned_correction_writes_the_new_values_and_an_action_log_entry(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool);
        let employer_id = an_employer(&db).await;
        let person_id = a_person(&db, &employer_id, "Ada Lovelace").await;
        set_person_particulars(
            &db,
            &employer_id,
            &person_id,
            fields("12345678901"),
            &[],
            "",
            "operator:alice",
        )
        .await
        .unwrap();

        let diverging = set_person_particulars(
            &db,
            &employer_id,
            &person_id,
            fields("98765432109"),
            &[],
            "typo in the identity number",
            "operator:alice",
        )
        .await
        .unwrap();

        assert_eq!(diverging, Vec::new());
        let logged: (String, String, serde_json::Value) = sqlx::query_as(
            "SELECT actor, action_type, context FROM action_log_entry WHERE target_id = $1",
        )
        .bind(person_id.as_str())
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(logged.0, "operator:alice");
        assert_eq!(logged.1, "person_particulars_corrected");
        assert_eq!(logged.2["reason"], "typo in the identity number");
        assert_eq!(logged.2["before"]["identity_number"], "12345678901");
        assert_eq!(logged.2["after"]["identity_number"], "98765432109");
    }

    #[sqlx::test]
    async fn a_blank_full_name_correction_is_refused(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool);
        let employer_id = an_employer(&db).await;
        let person_id = a_person(&db, &employer_id, "Ada Lovelace").await;

        let err = correct_person_full_name(
            &db,
            &employer_id,
            &person_id,
            "   ",
            &[],
            "fixing a typo",
            "operator:alice",
        )
        .await
        .unwrap_err();

        assert_eq!(err, PayrollAppError::PersonFullNameCannotBeEmpty);
    }

    #[sqlx::test]
    async fn a_full_name_correction_demands_a_reason_unconditionally(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool);
        let employer_id = an_employer(&db).await;
        let person_id = a_person(&db, &employer_id, "Ada Lovelace").await;

        let err = correct_person_full_name(
            &db,
            &employer_id,
            &person_id,
            "Ada Byron",
            &[],
            "",
            "operator:alice",
        )
        .await
        .unwrap_err();

        assert_eq!(
            err,
            PayrollAppError::PersonNameCorrectionReasonCannotBeEmpty
        );
        assert_eq!(
            get_person_particulars(&db, &employer_id, &person_id)
                .await
                .unwrap()
                .full_name,
            "Ada Lovelace"
        );
    }

    #[sqlx::test]
    async fn a_reasoned_full_name_correction_is_trimmed_and_logged(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool);
        let employer_id = an_employer(&db).await;
        let person_id = a_person(&db, &employer_id, "Ada Lovelaec").await;

        let diverging = correct_person_full_name(
            &db,
            &employer_id,
            &person_id,
            "  Ada Lovelace  ",
            &[],
            "fixing a misspelling",
            "operator:alice",
        )
        .await
        .unwrap();

        assert_eq!(diverging, Vec::new());
        assert_eq!(
            get_person_particulars(&db, &employer_id, &person_id)
                .await
                .unwrap()
                .full_name,
            "Ada Lovelace"
        );

        let logged: (String, serde_json::Value) = sqlx::query_as(
            "SELECT action_type, context FROM action_log_entry WHERE target_id = $1",
        )
        .bind(person_id.as_str())
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(logged.0, "person_full_name_corrected");
        assert_eq!(logged.1["before"]["full_name"], "Ada Lovelaec");
        assert_eq!(logged.1["after"]["full_name"], "Ada Lovelace");
    }

    // The "diverges from live finalized payroll" cases, and the "a Legacy
    // Person is correctable" acceptance criterion, need a real finalized run
    // and the migration-created legacy row respectively — both exercised in
    // `crates/payroll-app/tests/person_particulars.rs` alongside the
    // HTTP-level tests, rather than duplicating that setup here.
}
