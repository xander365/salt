//! `EmployerParticulars` (issue #71, parent #70 D-7; CONTEXT.md). The
//! Employer's registered name, address and statutory registration numbers —
//! one row per Employer, Owner-only to write, correctable forever with a
//! stated reason. Not effective-dated: unlike `CompensationTerms`, there is
//! no `effective_from` to split "record a new one" from "correct the
//! existing one" by, so both acts collapse into one upsert,
//! [`set_employer_particulars`], decided by whether a row already exists.
//!
//! Nothing here freezes into a `FinalizedPayroll` — that is #70 D-7's own
//! later ticket, out of this one's scope — so this table is read live by
//! whatever renders a payslip until then.

use chrono::{DateTime, Utc};
use payroll::{EmployerId, PayPeriod};

use crate::action_log::{ActionLogEntry, ActionType, write_action_log_entry};
use crate::database::SaltDatabase;
use crate::error::PayrollAppError;
use crate::freeze::{
    diverging_periods_json, live_finalized_periods_for_employer,
    require_acknowledgement_of_employer,
};

/// One `EmployerParticulars` row, read back by [`get_employer_particulars`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmployerParticulars {
    pub registered_name: String,
    pub address_line1: String,
    pub address_line2: Option<String>,
    pub city: String,
    pub postal_code: Option<String>,
    pub income_tax_number: Option<String>,
    pub social_security_number: Option<String>,
    pub created_at: DateTime<Utc>,
    pub created_by: String,
}

/// The fields [`set_employer_particulars`] writes, named once so the use
/// case and its ActionLog "before"/"after" JSON build from the same shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmployerParticularsFields {
    pub registered_name: String,
    pub address_line1: String,
    pub address_line2: Option<String>,
    pub city: String,
    pub postal_code: Option<String>,
    pub income_tax_number: Option<String>,
    pub social_security_number: Option<String>,
}

fn fields_json(fields: &EmployerParticularsFields) -> serde_json::Value {
    serde_json::json!({
        "registered_name": fields.registered_name,
        "address_line1": fields.address_line1,
        "address_line2": fields.address_line2,
        "city": fields.city,
        "postal_code": fields.postal_code,
        "income_tax_number": fields.income_tax_number,
        "social_security_number": fields.social_security_number,
    })
}

/// The seven columns [`set_employer_particulars`] reads back before it
/// overwrites them, in the order the `SELECT ... FOR UPDATE` below names
/// them. Converted straight into [`EmployerParticularsFields`] so the
/// ActionLog's "before" is built by the same [`fields_json`] that builds its
/// "after": one shape, one builder, and no way for the two halves of a
/// correction record to drift apart.
type StoredFields = (
    String,
    String,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

impl From<StoredFields> for EmployerParticularsFields {
    fn from(stored: StoredFields) -> Self {
        let (
            registered_name,
            address_line1,
            address_line2,
            city,
            postal_code,
            income_tax_number,
            social_security_number,
        ) = stored;
        Self {
            registered_name,
            address_line1,
            address_line2,
            city,
            postal_code,
            income_tax_number,
            social_security_number,
        }
    }
}

/// An optional field that arrives holding nothing but whitespace states
/// nothing, and is stored as the absence it is. This crate owns the
/// invariant rather than leaving it to `salt-server`'s own wire mapping
/// (ADR-0018): the migration's `..._not_blank_if_present` CHECKs are the
/// last line, and a use case that let a whitespace-only value reach them
/// would turn a refusable input into a 500.
fn normalize_optional_fields(fields: EmployerParticularsFields) -> EmployerParticularsFields {
    fn stated(value: Option<String>) -> Option<String> {
        value.filter(|value| !value.trim().is_empty())
    }

    EmployerParticularsFields {
        address_line2: stated(fields.address_line2),
        postal_code: stated(fields.postal_code),
        income_tax_number: stated(fields.income_tax_number),
        social_security_number: stated(fields.social_security_number),
        ..fields
    }
}

/// The frozen snapshot [`crate::finalize::finalize_payroll_run`] writes onto
/// every member's `FinalizedPayroll` for `employer_id` (issue #73,
/// CONTEXT.md's own glossary entry) — `None` when this Employer has never
/// recorded particulars, because there is nothing true to freeze (ADR-0004):
/// a `FinalizedPayroll` freezes the values an Employer actually had on
/// record, never a value derived from what it has today. Reuses
/// [`fields_json`], the exact shape [`set_employer_particulars`]'s own
/// ActionLog "after" already builds, so the frozen snapshot and a live
/// correction's own record of itself can never describe the same row two
/// different ways.
///
/// Takes a plain connection and locks nothing itself:
/// `finalize_payroll_run` calls this only after its own `FOR SHARE` on the
/// `employer` row (via `pay_schedule_for_employer`, `freeze.rs`'s module
/// doc), and that is what serializes this read against a concurrent
/// `set_employer_particulars`, which takes `FOR UPDATE` on that same row
/// before it ever writes this table.
pub(crate) async fn employer_particulars_snapshot(
    conn: &mut sqlx::PgConnection,
    employer_id: &EmployerId,
) -> Result<Option<serde_json::Value>, PayrollAppError> {
    let existing: Option<StoredFields> = sqlx::query_as(
        "SELECT registered_name, address_line1, address_line2, city, postal_code,
                income_tax_number, social_security_number
         FROM employer_particulars
         WHERE employer_id = $1",
    )
    .bind(employer_id.as_str())
    .fetch_optional(conn)
    .await?;

    Ok(existing
        .map(EmployerParticularsFields::from)
        .as_ref()
        .map(fields_json))
}

/// `GET`'s own read: `None` when this Employer has never recorded
/// particulars.
pub async fn get_employer_particulars(
    db: &SaltDatabase,
    employer_id: &EmployerId,
) -> Result<Option<EmployerParticulars>, PayrollAppError> {
    type Row = (
        String,
        String,
        Option<String>,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        DateTime<Utc>,
        String,
    );
    let row: Option<Row> = sqlx::query_as(
        "SELECT registered_name, address_line1, address_line2, city, postal_code,
                income_tax_number, social_security_number, created_at, created_by
         FROM employer_particulars
         WHERE employer_id = $1",
    )
    .bind(employer_id.as_str())
    .fetch_optional(db.pool())
    .await?;

    Ok(row.map(
        |(
            registered_name,
            address_line1,
            address_line2,
            city,
            postal_code,
            income_tax_number,
            social_security_number,
            created_at,
            created_by,
        )| EmployerParticulars {
            registered_name,
            address_line1,
            address_line2,
            city,
            postal_code,
            income_tax_number,
            social_security_number,
            created_at,
            created_by,
        },
    ))
}

/// Records or corrects `employer_id`'s `EmployerParticulars` — an insert
/// when none exists yet, an update when one does, decided inside this
/// call's own transaction. Owner-only at the HTTP boundary
/// (`AuthorizedEmployerContext::require_role`, issue #71's own "first
/// Owner-only route"); this use case trusts its caller about that, the same
/// as every other `payroll-app` use case trusts `salt-server` to have
/// authorized the actor before it is reached (ADR-0017, ADR-0019).
///
/// Reuses `correct_compensation_terms`'s own correction pattern (§6.5),
/// generalized to a fact with no `effective_from` to derive a span from: the
/// divergence this write is checked against is every Live finalized
/// `PayPeriod` this Employer has, not a dated slice of them
/// ([`live_finalized_periods_for_employer`]) — the same warning-never-a-
/// refusal contract, the same acknowledgement guard, and one
/// `EmployerParticularsCorrected` `ActionLog` entry as the record of the
/// acknowledgement (no second sign-off table).
///
/// `reason` is demanded differently depending on which act this write turns
/// out to be, mirroring `correct_compensation_terms` and
/// `record_compensation_terms` respectively:
///
/// * Correcting an existing row demands a non-blank `reason`
///   unconditionally, checked before the divergence list is even computed —
///   the same demand `correct_compensation_terms` makes of every correction.
/// * Recording one for the first time demands it only once the divergence
///   list comes back non-empty — an insert that diverges from nothing is the
///   ordinary act of finally filling in an Employer's details, and owes no
///   explanation.
pub async fn set_employer_particulars(
    db: &SaltDatabase,
    employer_id: &EmployerId,
    fields: EmployerParticularsFields,
    acknowledged_diverging_periods: &[PayPeriod],
    reason: &str,
    actor: &str,
) -> Result<Vec<PayPeriod>, PayrollAppError> {
    let fields = normalize_optional_fields(fields);

    if fields.registered_name.trim().is_empty() {
        return Err(PayrollAppError::EmployerParticularsRegisteredNameCannotBeEmpty);
    }
    if fields.address_line1.trim().is_empty() {
        return Err(PayrollAppError::EmployerParticularsAddressLine1CannotBeEmpty);
    }
    if fields.city.trim().is_empty() {
        return Err(PayrollAppError::EmployerParticularsCityCannotBeEmpty);
    }

    let mut tx = db.pool().begin().await?;

    // `FOR UPDATE` on the Employer row: it is what makes the divergence list
    // read below hold against a concurrent `finalize_payroll_run`, which
    // takes its own `FOR SHARE` on this row before it reads any master data
    // (`freeze.rs`'s own module doc).
    let exists: Option<String> =
        sqlx::query_scalar("SELECT id FROM employer WHERE id = $1 FOR UPDATE")
            .bind(employer_id.as_str())
            .fetch_optional(&mut *tx)
            .await?;
    if exists.is_none() {
        return Err(PayrollAppError::EmployerNotFound(employer_id.clone()));
    }

    // `FOR UPDATE` on any existing particulars row too, serializing two
    // concurrent writes to it against each other the same way
    // `correct_compensation_terms` locks the row it corrects.
    let existing: Option<StoredFields> = sqlx::query_as(
        "SELECT registered_name, address_line1, address_line2, city, postal_code,
                income_tax_number, social_security_number
         FROM employer_particulars
         WHERE employer_id = $1
         FOR UPDATE",
    )
    .bind(employer_id.as_str())
    .fetch_optional(&mut *tx)
    .await?;
    let is_correction = existing.is_some();

    // A correction demands its reason unconditionally, before the
    // divergence list is even computed — the same order
    // `correct_compensation_terms` checks in.
    if is_correction && reason.trim().is_empty() {
        return Err(PayrollAppError::EmployerParticularsCorrectionReasonCannotBeEmpty);
    }

    let diverging_periods = live_finalized_periods_for_employer(&mut tx, employer_id).await?;

    require_acknowledgement_of_employer(
        employer_id,
        &diverging_periods,
        acknowledged_diverging_periods,
    )?;

    let diverges = !diverging_periods.is_empty();
    if !is_correction && diverges && reason.trim().is_empty() {
        return Err(PayrollAppError::EmployerParticularsCorrectionReasonCannotBeEmpty);
    }

    if is_correction {
        sqlx::query(
            "UPDATE employer_particulars
             SET registered_name = $2, address_line1 = $3, address_line2 = $4, city = $5,
                 postal_code = $6, income_tax_number = $7, social_security_number = $8
             WHERE employer_id = $1",
        )
        .bind(employer_id.as_str())
        .bind(&fields.registered_name)
        .bind(&fields.address_line1)
        .bind(&fields.address_line2)
        .bind(&fields.city)
        .bind(&fields.postal_code)
        .bind(&fields.income_tax_number)
        .bind(&fields.social_security_number)
        .execute(&mut *tx)
        .await?;
    } else {
        sqlx::query(
            "INSERT INTO employer_particulars
                (employer_id, registered_name, address_line1, address_line2, city, postal_code,
                 income_tax_number, social_security_number, created_by)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(employer_id.as_str())
        .bind(&fields.registered_name)
        .bind(&fields.address_line1)
        .bind(&fields.address_line2)
        .bind(&fields.city)
        .bind(&fields.postal_code)
        .bind(&fields.income_tax_number)
        .bind(&fields.social_security_number)
        .bind(actor)
        .execute(&mut *tx)
        .await?;
    }

    // Logged whenever this write corrects something already recorded, or
    // inserts a row that diverges from live finalized payroll — the same
    // "only a diverging insert is a correction" rule
    // `record_compensation_terms` applies to its own ActionLog entry.
    if is_correction || diverges {
        write_action_log_entry(
            &mut tx,
            ActionLogEntry {
                employer_id,
                actor,
                action_type: ActionType::EmployerParticularsCorrected,
                target_type: "employer",
                target_id: employer_id.as_str(),
                context: Some(serde_json::json!({
                    "reason": reason,
                    // `null` for a first-ever diverging record: there was no
                    // row governed under this key before now, and the
                    // absence is recorded as an absence rather than as a
                    // fabricated value (the same choice
                    // `record_compensation_terms` makes of its own insert).
                    "before": existing
                        .map(EmployerParticularsFields::from)
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

#[cfg(test)]
mod tests {
    use payroll::{DayOfMonth, PaySchedule, PeriodEndDay};
    use sqlx::PgPool;

    use super::*;
    use crate::employer::create_employer;

    fn fields(registered_name: &str) -> EmployerParticularsFields {
        EmployerParticularsFields {
            registered_name: registered_name.to_string(),
            address_line1: "1 Independence Ave".to_string(),
            address_line2: None,
            city: "Windhoek".to_string(),
            postal_code: Some("10001".to_string()),
            income_tax_number: Some("12345678".to_string()),
            social_security_number: None,
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

    #[sqlx::test]
    async fn no_particulars_read_back_as_none(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool);
        let employer_id = an_employer(&db).await;

        assert_eq!(
            get_employer_particulars(&db, &employer_id).await.unwrap(),
            None
        );
    }

    #[sqlx::test]
    async fn a_first_record_with_nothing_to_diverge_from_needs_no_reason(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool);
        let employer_id = an_employer(&db).await;

        let diverging = set_employer_particulars(
            &db,
            &employer_id,
            fields("Acme Corp (Pty) Ltd"),
            &[],
            "",
            "operator:alice",
        )
        .await
        .unwrap();

        assert_eq!(diverging, Vec::new());
        let stored = get_employer_particulars(&db, &employer_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.registered_name, "Acme Corp (Pty) Ltd");
        assert_eq!(stored.created_by, "operator:alice");
    }

    #[sqlx::test]
    async fn a_blank_registered_name_is_refused(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool);
        let employer_id = an_employer(&db).await;

        let err =
            set_employer_particulars(&db, &employer_id, fields("   "), &[], "", "operator:alice")
                .await
                .unwrap_err();

        assert_eq!(
            err,
            PayrollAppError::EmployerParticularsRegisteredNameCannotBeEmpty
        );
    }

    #[sqlx::test]
    async fn a_whitespace_only_optional_field_is_stored_as_the_absence_it_is(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool);
        let employer_id = an_employer(&db).await;

        set_employer_particulars(
            &db,
            &employer_id,
            EmployerParticularsFields {
                address_line2: Some("   ".to_string()),
                postal_code: Some("\t".to_string()),
                income_tax_number: Some(String::new()),
                social_security_number: Some(" \n ".to_string()),
                ..fields("Acme Corp")
            },
            &[],
            "",
            "operator:alice",
        )
        .await
        .unwrap();

        // Not a CHECK violation surfacing as an unexpected database error:
        // the use case normalized all four before they reached the row.
        let stored = get_employer_particulars(&db, &employer_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.address_line2, None);
        assert_eq!(stored.postal_code, None);
        assert_eq!(stored.income_tax_number, None);
        assert_eq!(stored.social_security_number, None);
    }

    #[sqlx::test]
    async fn an_unknown_employer_is_refused(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool);

        let err = set_employer_particulars(
            &db,
            &EmployerId::new("no-such-employer"),
            fields("Acme Corp"),
            &[],
            "",
            "operator:alice",
        )
        .await
        .unwrap_err();

        assert_eq!(
            err,
            PayrollAppError::EmployerNotFound(EmployerId::new("no-such-employer"))
        );
    }

    #[sqlx::test]
    async fn correcting_an_existing_row_demands_a_reason_even_with_nothing_to_diverge_from(
        pool: PgPool,
    ) {
        let db = SaltDatabase::from_pool(pool);
        let employer_id = an_employer(&db).await;
        set_employer_particulars(
            &db,
            &employer_id,
            fields("Acme Corp"),
            &[],
            "",
            "operator:alice",
        )
        .await
        .unwrap();

        let err = set_employer_particulars(
            &db,
            &employer_id,
            fields("Acme Holdings"),
            &[],
            "",
            "operator:alice",
        )
        .await
        .unwrap_err();

        assert_eq!(
            err,
            PayrollAppError::EmployerParticularsCorrectionReasonCannotBeEmpty
        );
        // Refused before anything was touched.
        assert_eq!(
            get_employer_particulars(&db, &employer_id)
                .await
                .unwrap()
                .unwrap()
                .registered_name,
            "Acme Corp"
        );
    }

    #[sqlx::test]
    async fn a_reasoned_correction_writes_the_new_values_and_an_action_log_entry(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool);
        let employer_id = an_employer(&db).await;
        set_employer_particulars(
            &db,
            &employer_id,
            fields("Acme Corp"),
            &[],
            "",
            "operator:alice",
        )
        .await
        .unwrap();

        let diverging = set_employer_particulars(
            &db,
            &employer_id,
            fields("Acme Holdings"),
            &[],
            "registered new legal name",
            "operator:alice",
        )
        .await
        .unwrap();

        assert_eq!(diverging, Vec::new());
        assert_eq!(
            get_employer_particulars(&db, &employer_id)
                .await
                .unwrap()
                .unwrap()
                .registered_name,
            "Acme Holdings"
        );

        let logged: (String, String, serde_json::Value) = sqlx::query_as(
            "SELECT actor, action_type, context FROM action_log_entry WHERE employer_id = $1",
        )
        .bind(employer_id.as_str())
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(logged.0, "operator:alice");
        assert_eq!(logged.1, "employer_particulars_corrected");
        assert_eq!(logged.2["reason"], "registered new legal name");
        assert_eq!(logged.2["before"]["registered_name"], "Acme Corp");
        assert_eq!(logged.2["after"]["registered_name"], "Acme Holdings");
    }

    // The "diverges from live finalized payroll" cases need a real
    // finalized run to diverge from, which needs an Employment, its
    // compensation terms, and a calculated-then-finalized PayrollRun beside
    // it — the full setup `crates/payroll-app/tests/master_data_correction.rs`
    // already builds and reuses for `record_compensation_terms` and
    // `declare_unsupported_deduction_status`'s own divergence tests. Those
    // cases for `set_employer_particulars` live there beside them, rather
    // than duplicating that setup here.
}
