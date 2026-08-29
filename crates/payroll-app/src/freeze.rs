//! The freeze guard ADR-0013 names: a fact that later periods re-read
//! becomes immutable at "that Employment's first finalization in that
//! TaxYear" (§4.5, §4.2, issue #33). `finalized_payroll` never loses a row —
//! `reverse_finalized_payroll` only ever deletes the `live_finalized_payroll`
//! liveness row (migration 0015 revokes UPDATE and DELETE on
//! `finalized_payroll` itself) — so a plain existence check against it
//! already counts Live and reversed alike, and a later reversal can never
//! thaw a fact this once made frozen.

use payroll::{EmployerId, EmploymentId, TaxYear};

/// Whether `employment_id` has any `FinalizedPayroll` — Live or reversed —
/// in `tax_year`. The freeze trigger for `OpeningBalance` and
/// `PriorEmployment` (§4.5, §4.5b): both are Employment+TaxYear state, so
/// this is exactly "has this row's own TaxYear already been finalized for
/// this Employment".
pub(crate) async fn employment_has_a_finalization_in(
    conn: &mut sqlx::PgConnection,
    employment_id: &EmploymentId,
    tax_year: TaxYear,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM finalized_payroll
            WHERE employment_id = $1 AND tax_year = $2
        )",
    )
    .bind(employment_id.as_str())
    .bind(tax_year.starting_year())
    .fetch_one(conn)
    .await
}

/// Whether any Employment of `employer_id` has a `FinalizedPayroll` — Live
/// or reversed — in `tax_year`. The freeze trigger for a `PaySchedule`
/// change (§4.2): the schedule is Employer-level, so one finalized payroll
/// anywhere in the TaxYear is enough to lock it in, whichever Employment it
/// belongs to.
pub(crate) async fn employer_has_a_finalization_in(
    conn: &mut sqlx::PgConnection,
    employer_id: &EmployerId,
    tax_year: TaxYear,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM finalized_payroll
            WHERE employer_id = $1 AND tax_year = $2
        )",
    )
    .bind(employer_id.as_str())
    .bind(tax_year.starting_year())
    .fetch_one(conn)
    .await
}
