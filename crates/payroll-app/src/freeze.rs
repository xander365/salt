//! The freeze guard ADR-0013 names: a fact that later periods re-read
//! becomes immutable at "that Employment's first finalization in that
//! TaxYear" (§4.5, §4.2, issue #33). `finalized_payroll` never loses a row —
//! `reverse_finalized_payroll` only ever deletes the `live_finalized_payroll`
//! liveness row (migration 0015 revokes UPDATE and DELETE on
//! `finalized_payroll` itself) — so a plain existence check against it
//! already counts Live and reversed alike, and a later reversal can never
//! thaw a fact this once made frozen.
//!
//! The three queries below are deliberately written out rather than folded
//! into one: they name two different columns and two different comparisons,
//! and this crate builds no SQL by string interpolation. Three static
//! statements behind three names that read at the call site cost less than a
//! `match` that hands back a query string.
//!
//! None of them takes a lock. Each is a *check*, and the lock that makes the
//! check hold is the caller's own — `FOR UPDATE` on the `employment` or
//! `employer` row the frozen fact hangs off. `finalize_payroll_run` takes
//! `FOR SHARE` on those same rows before it reads any master data, and `FOR
//! SHARE` conflicts with `FOR UPDATE`, so a finalization and an edit of a
//! frozen fact can never interleave: whichever arrives second waits, then
//! sees the other's committed result and refuses.

use chrono::NaiveDate;
use payroll::{EmployerId, EmploymentId, PayPeriod, TaxYear};

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
/// or reversed — in `tax_year` **or any TaxYear after it**. The freeze
/// trigger for a `PaySchedule` change (§4.2), where `tax_year` is the
/// caller's own claim about which TaxYear the change is being made in: a
/// claim naming a TaxYear earlier than one that has already finalized
/// payroll is refused by the `>=` rather than waved through.
pub(crate) async fn employer_has_a_finalization_in_or_after(
    conn: &mut sqlx::PgConnection,
    employer_id: &EmployerId,
    tax_year: TaxYear,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM finalized_payroll
            WHERE employer_id = $1 AND tax_year >= $2
        )",
    )
    .bind(employer_id.as_str())
    .bind(tax_year.starting_year())
    .fetch_one(conn)
    .await
}

/// Every distinct period end this Employer has already finalized in
/// `tax_year`, Live or reversed. `create_ordinary_payroll_run` checks these
/// against the Employer's *current* `PaySchedule`: a period end the schedule
/// no longer generates is proof the schedule moved inside a TaxYear that had
/// already finalized payroll, whatever TaxYear the change claimed to be made
/// in (§4.2, ADR-0005).
pub(crate) async fn finalized_period_ends_in(
    conn: &mut sqlx::PgConnection,
    employer_id: &EmployerId,
    tax_year: TaxYear,
) -> Result<Vec<NaiveDate>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT DISTINCT period_end FROM finalized_payroll
         WHERE employer_id = $1 AND tax_year = $2
         ORDER BY period_end",
    )
    .bind(employer_id.as_str())
    .bind(tax_year.starting_year())
    .fetch_all(conn)
    .await
}

/// Every Live `FinalizedPayroll` `PayPeriod` for `employment_id` whose
/// `period_end` falls in `[from, until)` — `until = None` meaning
/// open-ended — read back in `period_end` order (§6.5).
///
/// This is the divergence a master-data correction into that span produces,
/// and it is shared by `correct_compensation_terms` and
/// `declare_unsupported_deduction_status`: what differs between the two
/// facts is only how `from`/`until` — the span the row being written
/// currently governs — are derived from their own table's sibling rows.
/// Reading Live finalized periods back is the one part that never differs,
/// so it lives here rather than being written out twice.
pub(crate) async fn live_finalized_periods_in_span(
    conn: &mut sqlx::PgConnection,
    employment_id: &EmploymentId,
    from: NaiveDate,
    until: Option<NaiveDate>,
) -> Result<Vec<PayPeriod>, sqlx::Error> {
    let rows: Vec<(NaiveDate, NaiveDate)> = sqlx::query_as(
        "SELECT finalized.period_start, finalized.period_end
         FROM live_finalized_payroll AS live
         JOIN finalized_payroll AS finalized ON finalized.id = live.finalized_payroll_id
         WHERE live.employment_id = $1
           AND live.period_end >= $2
           AND ($3::date IS NULL OR live.period_end < $3)
         ORDER BY live.period_end",
    )
    .bind(employment_id.as_str())
    .bind(from)
    .bind(until)
    .fetch_all(conn)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(start, end)| {
            PayPeriod::new(start, end)
                .expect("finalized_payroll CHECK: period_end is never before period_start")
        })
        .collect())
}
