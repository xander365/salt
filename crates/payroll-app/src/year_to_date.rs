//! `BuildYearToDateContext` (§8, §12): the `YearToDateContext` for one
//! Employment as of a `PayPeriod` end date, assembled entirely from stored
//! history — never retyped, and never a running total (INV-013).

use chrono::NaiveDate;
use payroll::{EmploymentId, Money, PayrollError, PeriodsElapsed, TaxYear, YearToDateContext};
use sqlx::{Acquire, Postgres};

use crate::database::SaltDatabase;
use crate::error::PayrollAppError;
use crate::prior_employment::get_prior_employment_on;

/// Builds the `YearToDateContext` for `employment_id` as of `period_end` —
/// the end date of the `PayPeriod` being calculated (§8):
///
/// - `tax_year` and `periods_elapsed` are derived from `period_end` alone,
///   through the pure crate's own derivations (`TaxYear::for_period_end`,
///   `PeriodsElapsed::from_period_end`). Neither counts a row — counting
///   periods paid over-withholds every mid-year joiner, the failure
///   `payroll::year_to_date` documents at length.
/// - `prior_taxable_remuneration` and `prior_paye` are the Employment's
///   `OpeningBalance` figures for the TaxYear — zero where none is
///   recorded, a consequence of §7's walk-back having already proved every
///   earlier period is resolved, not an assumption from silence — plus the
///   sum of `taxable_remuneration`/`paye` from every *live* `FinalizedPayroll`
///   row for the same Employment and TaxYear whose `period_end` is earlier
///   than `period_end`.
/// - `prior_employment` is read back through `get_prior_employment`, the
///   one term with no stored-history fallback (§4.5b): an absent
///   declaration is `Unknown`, not zero.
///
/// "Live" means joined through `live_finalized_payroll`: the query never
/// mentions `reversal`, and a reversed-then-unreplaced period simply drops
/// out of the sum (§6.2, §7.1 branch 4). There is no `FinalizedPayroll`
/// history to sum yet — finalization and reversal are separate, later
/// tickets that fill `live_finalized_payroll` with rows — so this join is
/// written now, to be exercised for real once they ship, rather than
/// rewritten later.
///
/// Public entry point over the opaque [`SaltDatabase`] handle. The
/// generic connection-taking implementation is
/// `build_year_to_date_context_on`, used internally by `calculate.rs`,
/// which already holds a transaction and needs this read on that same
/// connection.
pub async fn build_year_to_date_context(
    db: &SaltDatabase,
    employment_id: &EmploymentId,
    period_end: NaiveDate,
) -> Result<YearToDateContext, PayrollAppError> {
    build_year_to_date_context_on(db.pool(), employment_id, period_end).await
}

/// Takes anything a connection can be acquired from — a `&PgPool` for a
/// standalone read, or a `&mut Transaction` so a caller assembling several
/// facts at once reads them all on the one connection, inside its own
/// transaction and under whatever lock it already holds. All three reads
/// below share that connection, so the `OpeningBalance`, the live history
/// and the `PriorEmployment` are one consistent account of the TaxYear
/// rather than three snapshots taken moments apart.
pub(crate) async fn build_year_to_date_context_on<'a>(
    conn: impl Acquire<'a, Database = Postgres>,
    employment_id: &EmploymentId,
    period_end: NaiveDate,
) -> Result<YearToDateContext, PayrollAppError> {
    let mut conn = conn.acquire().await?;

    let tax_year = TaxYear::for_period_end(period_end);
    let periods_elapsed = PeriodsElapsed::from_period_end(period_end);
    let prior_employment = get_prior_employment_on(&mut *conn, employment_id, tax_year).await?;

    let opening: Option<(i64, i64)> = sqlx::query_as(
        "SELECT prior_taxable_remuneration, prior_paye
         FROM opening_balance
         WHERE employment_id = $1 AND tax_year = $2",
    )
    .bind(employment_id.as_str())
    .bind(tax_year.starting_year())
    .fetch_optional(&mut *conn)
    .await?;
    let (opening_taxable_cents, opening_paye_cents) = opening.unwrap_or((0, 0));

    // `finalized_payroll.taxable_remuneration`/`.paye` are BIGINT cents,
    // non-negative, by migration 0024 — the same hardening 0020 and 0021
    // gave the other Money columns. PostgreSQL's `SUM` over BIGINT still
    // widens to NUMERIC to keep the running total from overflowing, so the
    // cast back is what names the result's type; the CHECK is what makes it
    // exact, never a rounding shortcut.
    let (live_taxable_cents, live_paye_cents): (i64, i64) = sqlx::query_as(
        "SELECT COALESCE(SUM(finalized.taxable_remuneration), 0)::bigint,
                COALESCE(SUM(finalized.paye), 0)::bigint
         FROM live_finalized_payroll AS live
         JOIN finalized_payroll AS finalized ON finalized.id = live.finalized_payroll_id
         WHERE live.employment_id = $1
           AND live.period_end < $2
           AND finalized.tax_year = $3",
    )
    .bind(employment_id.as_str())
    .bind(period_end)
    .bind(tax_year.starting_year())
    .fetch_one(&mut *conn)
    .await?;

    // Both sides are non-negative whole cents by CHECK (`opening_balance`
    // from 0021, `finalized_payroll` from 0024), so reconstructing a `Money`
    // cannot fail — the same schema-guaranteed `expect` every other reader
    // in this crate uses. Their *sum* is a different matter: a TaxYear's
    // worth of live history can genuinely exceed what a `Money` holds, and
    // that is a refusal to report rather than a stored-data fault.
    let money = |cents: i64| {
        Money::from_cents(cents)
            .expect("opening_balance and finalized_payroll CHECK: Money columns hold whole, non-negative cents")
    };
    let prior_taxable_remuneration = money(opening_taxable_cents)
        .checked_add(money(live_taxable_cents))
        .map_err(|_| PayrollAppError::from(PayrollError::AmountOverflow))?;
    let prior_paye = money(opening_paye_cents)
        .checked_add(money(live_paye_cents))
        .map_err(|_| PayrollAppError::from(PayrollError::AmountOverflow))?;

    Ok(YearToDateContext::new(
        tax_year,
        prior_taxable_remuneration,
        prior_paye,
        periods_elapsed,
        prior_employment,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{create_employer, create_employment, declare_prior_employment, void_employment};
    use payroll::{DayOfMonth, PeriodEndDay, PersonId, PriorEmployment};
    use sqlx::{PgPool, Row};

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    fn twenty_sixth_schedule() -> payroll::PaySchedule {
        payroll::PaySchedule::new(PeriodEndDay::Day(DayOfMonth::new(25).unwrap()))
    }

    async fn an_employment(db: &SaltDatabase) -> EmploymentId {
        let employer_id = create_employer(db, "Employer", twenty_sixth_schedule(), "actor")
            .await
            .unwrap();
        create_employment(
            db,
            &employer_id,
            &PersonId::new("person-1"),
            date(2020, 1, 1),
            None,
            "actor",
        )
        .await
        .unwrap()
    }

    #[sqlx::test]
    async fn with_no_opening_balance_and_no_finalized_history_the_prior_figures_are_zero(
        pool: PgPool,
    ) {
        let db = SaltDatabase::from_pool(pool.clone());
        let employment_id = an_employment(&db).await;
        declare_prior_employment(
            &db,
            &employment_id,
            TaxYear::starting(2025),
            PriorEmployment::None,
            "actor",
        )
        .await
        .unwrap();

        let ytd = build_year_to_date_context(&db, &employment_id, date(2026, 2, 25))
            .await
            .unwrap();

        assert_eq!(ytd.prior_taxable_remuneration(), Money::ZERO);
        assert_eq!(ytd.prior_paye(), Money::ZERO);
    }

    #[sqlx::test]
    async fn periods_elapsed_and_tax_year_are_derived_from_the_period_end_date_alone(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool.clone());
        let employment_id = an_employment(&db).await;
        declare_prior_employment(
            &db,
            &employment_id,
            TaxYear::starting(2026),
            PriorEmployment::None,
            "actor",
        )
        .await
        .unwrap();

        // September: six full months after the tax year's March start.
        let ytd = build_year_to_date_context(&db, &employment_id, date(2026, 9, 25))
            .await
            .unwrap();

        assert_eq!(ytd.tax_year(), TaxYear::starting(2026));
        assert_eq!(ytd.periods_elapsed(), PeriodsElapsed::new(6).unwrap());
    }

    #[sqlx::test]
    async fn the_opening_balance_figures_are_carried_through_when_no_history_precedes_them(
        pool: PgPool,
    ) {
        let db = SaltDatabase::from_pool(pool.clone());
        let employment_id = an_employment(&db).await;
        declare_prior_employment(
            &db,
            &employment_id,
            TaxYear::starting(2025),
            PriorEmployment::None,
            "actor",
        )
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO opening_balance
                (employment_id, tax_year, first_salt_period_end,
                 prior_taxable_remuneration, prior_paye, created_by)
             VALUES ($1, 2025, $2, 200000, 32000, 'actor')",
        )
        .bind(employment_id.as_str())
        .bind(date(2025, 9, 25))
        .execute(&pool)
        .await
        .unwrap();

        let ytd = build_year_to_date_context(&db, &employment_id, date(2025, 10, 25))
            .await
            .unwrap();

        assert_eq!(
            ytd.prior_taxable_remuneration(),
            Money::from_cents(200000).unwrap()
        );
        assert_eq!(ytd.prior_paye(), Money::from_cents(32000).unwrap());
    }

    /// A live `FinalizedPayroll` row for an earlier period end in the same
    /// TaxYear is summed on top of the `OpeningBalance`, reading the numeric
    /// columns and joining through liveness — never JSONB, never `reversal`.
    /// Written directly against the schema, since nothing in this ticket
    /// writes `finalized_payroll`/`live_finalized_payroll` yet (that is
    /// finalization's own, later ticket).
    #[sqlx::test]
    async fn a_live_finalized_payroll_row_for_an_earlier_period_is_summed_on_top(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool.clone());
        let employment_id = an_employment(&db).await;
        declare_prior_employment(
            &db,
            &employment_id,
            TaxYear::starting(2025),
            PriorEmployment::None,
            "actor",
        )
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO opening_balance
                (employment_id, tax_year, first_salt_period_end,
                 prior_taxable_remuneration, prior_paye, created_by)
             VALUES ($1, 2025, $2, 100000, 8000, 'actor')",
        )
        .bind(employment_id.as_str())
        .bind(date(2025, 9, 25))
        .execute(&pool)
        .await
        .unwrap();
        insert_live_finalized_payroll(
            &db,
            &employment_id,
            date(2025, 9, 26),
            date(2025, 10, 25),
            2025,
            1500000,
            100000,
        )
        .await;

        let ytd = build_year_to_date_context(&db, &employment_id, date(2025, 11, 25))
            .await
            .unwrap();

        assert_eq!(
            ytd.prior_taxable_remuneration(),
            Money::from_cents(100000 + 1500000).unwrap()
        );
        assert_eq!(ytd.prior_paye(), Money::from_cents(8000 + 100000).unwrap());
    }

    /// A row whose `period_end` is not earlier than the period being
    /// calculated must never be summed in — it is the same period or a
    /// later one, not year-to-date history.
    #[sqlx::test]
    async fn a_finalized_row_on_or_after_the_period_being_calculated_is_not_summed(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool.clone());
        let employment_id = an_employment(&db).await;
        declare_prior_employment(
            &db,
            &employment_id,
            TaxYear::starting(2025),
            PriorEmployment::None,
            "actor",
        )
        .await
        .unwrap();
        insert_live_finalized_payroll(
            &db,
            &employment_id,
            date(2025, 9, 26),
            date(2025, 10, 25),
            2025,
            1500000,
            100000,
        )
        .await;

        let ytd = build_year_to_date_context(&db, &employment_id, date(2025, 10, 25))
            .await
            .unwrap();

        assert_eq!(ytd.prior_taxable_remuneration(), Money::ZERO);
        assert_eq!(ytd.prior_paye(), Money::ZERO);
    }

    /// A period from a different TaxYear must never be summed in, even
    /// though it is a live row for the same Employment.
    #[sqlx::test]
    async fn a_finalized_row_from_a_different_tax_year_is_not_summed(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool.clone());
        let employment_id = an_employment(&db).await;
        declare_prior_employment(
            &db,
            &employment_id,
            TaxYear::starting(2026),
            PriorEmployment::None,
            "actor",
        )
        .await
        .unwrap();
        // 25 February 2026 falls in the TaxYear starting 2025, not 2026.
        insert_live_finalized_payroll(
            &db,
            &employment_id,
            date(2026, 1, 26),
            date(2026, 2, 25),
            2025,
            1500000,
            100000,
        )
        .await;

        let ytd = build_year_to_date_context(&db, &employment_id, date(2026, 3, 25))
            .await
            .unwrap();

        assert_eq!(ytd.prior_taxable_remuneration(), Money::ZERO);
        assert_eq!(ytd.prior_paye(), Money::ZERO);
    }

    /// A finalized row that has since been reversed, with nothing replacing
    /// it, has no `live_finalized_payroll` row any more — so it drops out
    /// of the sum with no query ever mentioning `reversal`.
    #[sqlx::test]
    async fn a_reversed_finalized_row_with_no_replacement_is_not_summed(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool.clone());
        let employment_id = an_employment(&db).await;
        declare_prior_employment(
            &db,
            &employment_id,
            TaxYear::starting(2025),
            PriorEmployment::None,
            "actor",
        )
        .await
        .unwrap();
        let finalized_payroll_id = insert_finalized_payroll(
            &db,
            &employment_id,
            date(2025, 9, 26),
            date(2025, 10, 25),
            2025,
            1500000,
            100000,
            date(2025, 10, 25),
        )
        .await;
        sqlx::query(
            "INSERT INTO reversal (finalized_payroll_id, reversed_by, reason)
             VALUES ($1::uuid, 'actor', 'wrong figure')",
        )
        .bind(&finalized_payroll_id)
        .execute(&pool)
        .await
        .unwrap();
        // No corresponding `live_finalized_payroll` row: reversal deletes
        // it, exactly as a later reversal ticket will do.

        let ytd = build_year_to_date_context(&db, &employment_id, date(2025, 11, 25))
            .await
            .unwrap();

        assert_eq!(ytd.prior_taxable_remuneration(), Money::ZERO);
        assert_eq!(ytd.prior_paye(), Money::ZERO);
    }

    /// §8: history is ordered by `PayPeriod` end date and never by when a
    /// record was finalized. A March payroll finalized in October is still
    /// March — so an earlier period finalized late is summed in, and a later
    /// period finalized early is left out, even though a `finalized_at`
    /// ordering would swap both answers.
    #[sqlx::test]
    async fn history_is_ordered_by_period_end_and_never_by_when_it_was_finalized(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool.clone());
        let employment_id = an_employment(&db).await;
        declare_prior_employment(
            &db,
            &employment_id,
            TaxYear::starting(2025),
            PriorEmployment::None,
            "actor",
        )
        .await
        .unwrap();
        // An earlier period, finalized months after the one being built.
        insert_live_finalized_payroll_finalized_on(
            &db,
            &employment_id,
            date(2025, 9, 26),
            date(2025, 10, 25),
            2025,
            1500000,
            100000,
            date(2026, 2, 20),
        )
        .await;
        // A later period, finalized long before it.
        insert_live_finalized_payroll_finalized_on(
            &db,
            &employment_id,
            date(2025, 11, 26),
            date(2025, 12, 25),
            2025,
            9900000,
            880000,
            date(2025, 9, 1),
        )
        .await;

        let ytd = build_year_to_date_context(&db, &employment_id, date(2025, 11, 25))
            .await
            .unwrap();

        assert_eq!(
            ytd.prior_taxable_remuneration(),
            Money::from_cents(1500000).unwrap()
        );
        assert_eq!(ytd.prior_paye(), Money::from_cents(100000).unwrap());
    }

    /// §8: `PeriodsElapsed` is the `PayPeriod`'s position in its TaxYear, and
    /// is never derived by counting finalized rows — counting is the single
    /// most tempting wrong reading, and it over-withholds from every mid-year
    /// joiner. Two live rows against a period seven months into the year
    /// tell the two readings apart.
    #[sqlx::test]
    async fn periods_elapsed_is_the_periods_position_and_never_a_count_of_history(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool.clone());
        let employment_id = an_employment(&db).await;
        declare_prior_employment(
            &db,
            &employment_id,
            TaxYear::starting(2026),
            PriorEmployment::None,
            "actor",
        )
        .await
        .unwrap();
        for (period_start, period_end) in [
            (date(2026, 7, 26), date(2026, 8, 25)),
            (date(2026, 8, 26), date(2026, 9, 25)),
        ] {
            insert_live_finalized_payroll(
                &db,
                &employment_id,
                period_start,
                period_end,
                2026,
                1500000,
                100000,
            )
            .await;
        }

        // 25 October 2026 sits seven full months after the TaxYear's March
        // start, whatever Salt happens to hold history for.
        let ytd = build_year_to_date_context(&db, &employment_id, date(2026, 10, 25))
            .await
            .unwrap();

        assert_eq!(ytd.periods_elapsed(), PeriodsElapsed::new(7).unwrap());
    }

    #[sqlx::test]
    async fn an_established_prior_employment_fact_is_carried_through(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool.clone());
        let employment_id = an_employment(&db).await;
        declare_prior_employment(
            &db,
            &employment_id,
            TaxYear::starting(2025),
            PriorEmployment::Some(payroll::PriorEmploymentFigures::new(
                Money::from_cents(50000).unwrap(),
                Money::from_cents(5000).unwrap(),
            )),
            "actor",
        )
        .await
        .unwrap();

        let ytd = build_year_to_date_context(&db, &employment_id, date(2026, 2, 25))
            .await
            .unwrap();

        assert_eq!(
            ytd.prior_employment(),
            PriorEmployment::Some(payroll::PriorEmploymentFigures::new(
                Money::from_cents(50000).unwrap(),
                Money::from_cents(5000).unwrap()
            ))
        );
    }

    /// No `PriorEmploymentDeclaration` row at all means `Unknown`, never a
    /// default of `None` — §4.5b's whole point.
    #[sqlx::test]
    async fn no_prior_employment_declaration_reads_back_as_unknown(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool.clone());
        let employment_id = an_employment(&db).await;

        let ytd = build_year_to_date_context(&db, &employment_id, date(2026, 2, 25))
            .await
            .unwrap();

        assert_eq!(ytd.prior_employment(), PriorEmployment::Unknown);
    }

    #[sqlx::test]
    async fn a_void_employment_is_refused(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool.clone());
        let employment_id = an_employment(&db).await;
        void_employment(&db, &employment_id, "actor").await.unwrap();

        let result = build_year_to_date_context(&db, &employment_id, date(2026, 2, 25)).await;

        assert_eq!(
            result,
            Err(PayrollAppError::EmploymentIsVoid(employment_id))
        );
    }

    async fn insert_live_finalized_payroll(
        db: &SaltDatabase,
        employment_id: &EmploymentId,
        period_start: NaiveDate,
        period_end: NaiveDate,
        tax_year: i32,
        taxable_remuneration_cents: i64,
        paye_cents: i64,
    ) {
        insert_live_finalized_payroll_finalized_on(
            db,
            employment_id,
            period_start,
            period_end,
            tax_year,
            taxable_remuneration_cents,
            paye_cents,
            // The ordinary case: finalized the day the period ended, so the
            // two dates agree and nothing rides on which one is read.
            period_end,
        )
        .await;
    }

    /// As above, but pins `finalized_at` separately from `period_end` — the
    /// one fixture that can tell §8's ordering rule apart from the timestamp
    /// it must never use.
    #[allow(clippy::too_many_arguments)]
    async fn insert_live_finalized_payroll_finalized_on(
        db: &SaltDatabase,
        employment_id: &EmploymentId,
        period_start: NaiveDate,
        period_end: NaiveDate,
        tax_year: i32,
        taxable_remuneration_cents: i64,
        paye_cents: i64,
        finalized_at: NaiveDate,
    ) {
        let finalized_payroll_id = insert_finalized_payroll(
            db,
            employment_id,
            period_start,
            period_end,
            tax_year,
            taxable_remuneration_cents,
            paye_cents,
            finalized_at,
        )
        .await;
        sqlx::query(
            "INSERT INTO live_finalized_payroll (employment_id, period_end, finalized_payroll_id)
             VALUES ($1, $2, $3::uuid)",
        )
        .bind(employment_id.as_str())
        .bind(period_end)
        .bind(&finalized_payroll_id)
        .execute(db.pool())
        .await
        .unwrap();
    }

    /// A `finalized_payroll` row must reference a real `payroll_run`, so
    /// this drives it through `create_ordinary_payroll_run` rather than
    /// inventing a run id — that keeps the fixture honest about what a
    /// `finalized_payroll` row always has behind it, even though nothing in
    /// this ticket calculates or finalizes that run.
    #[allow(clippy::too_many_arguments)]
    async fn insert_finalized_payroll(
        db: &SaltDatabase,
        employment_id: &EmploymentId,
        period_start: NaiveDate,
        period_end: NaiveDate,
        tax_year: i32,
        taxable_remuneration_cents: i64,
        paye_cents: i64,
        finalized_at: NaiveDate,
    ) -> String {
        let pool = db.pool();
        let employer_id: String =
            sqlx::query_scalar("SELECT employer_id FROM employment WHERE id = $1")
                .bind(employment_id.as_str())
                .fetch_one(pool)
                .await
                .unwrap();

        let run_id = crate::create_ordinary_payroll_run(
            db,
            &payroll::EmployerId::new(employer_id.clone()),
            payroll::PayPeriod::new(period_start, period_end).unwrap(),
            period_end,
            "actor",
        )
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO finalized_payroll
                (payroll_run_id, employment_id, employer_id, period_start, period_end, tax_year,
                 payroll_input_json, payroll_rules_json, payroll_calculation_json,
                 taxable_remuneration, paye, paye_table_id, ssc_rules_id, salt_version,
                 finalized_at, finalized_by)
             VALUES ($1::uuid, $2, $3, $4, $5, $6, '{}', '{}', '{}', $7, $8, 'x', 'x', 'x',
                     $9::date, 'actor')
             RETURNING id::text",
        )
        .bind(run_id.as_str())
        .bind(employment_id.as_str())
        .bind(&employer_id)
        .bind(period_start)
        .bind(period_end)
        .bind(tax_year)
        .bind(taxable_remuneration_cents)
        .bind(paye_cents)
        .bind(finalized_at)
        .fetch_one(pool)
        .await
        .unwrap()
        .get(0)
    }
}
