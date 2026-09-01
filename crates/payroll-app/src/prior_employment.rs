//! `DeclarePriorEmployment` and the read that resolves it (§4.5b, §12).

use payroll::{EmployerId, EmploymentId, Money, PriorEmployment, PriorEmploymentFigures, TaxYear};
use sqlx::{Acquire, Postgres};

use crate::action_log::{ActionLogEntry, ActionType, write_action_log_entry};
use crate::database::SaltDatabase;
use crate::error::PayrollAppError;
use crate::freeze::employment_has_a_finalization_in;

/// Records the `PriorEmploymentDeclaration` for one (Employment, TaxYear),
/// or replaces whichever one is already there. Concurrent employment is out
/// of scope (§4.5b), so this is Employment+TaxYear state — never
/// effective-dated and never run-scoped, because the fact it states cannot
/// change during the year.
///
/// `prior_employment` must be `None` or `Some`: the row itself is
/// two-valued, and `Unknown` is what the absence of a row already means
/// (§4.5b), so it is refused here rather than written.
///
/// `calculate` already refuses `Unknown`, so a run missing this declaration
/// can never reach `Calculated` (§4.5b). Frozen once the Employment's first
/// finalization in that TaxYear has happened (ADR-0013, issue #33): a
/// `PriorEmployment` declaration is re-read into every later period's
/// YearToDateContext, so an edit after that point would re-price
/// already-finalized figures silently.
pub async fn declare_prior_employment(
    db: &SaltDatabase,
    employment_id: &EmploymentId,
    tax_year: TaxYear,
    prior_employment: PriorEmployment,
    declared_by: &str,
) -> Result<(), PayrollAppError> {
    let (status, taxable_remuneration, paye) = match prior_employment {
        PriorEmployment::Unknown => {
            return Err(PayrollAppError::PriorEmploymentDeclarationCannotBeUnknown);
        }
        PriorEmployment::None => ("confirmed_none", None, None),
        PriorEmployment::Some(figures) => (
            "present",
            Some(figures.taxable_remuneration().cents()),
            Some(figures.paye().cents()),
        ),
    };

    let mut tx = db.pool().begin().await?;

    // `FOR UPDATE` holds the row against a concurrent `void_employment`, so
    // a void committing between this read and the insert cannot leave a
    // declaration recorded against a now-voided Employment. It is the
    // stronger lock for the same reason `record_opening_balance` takes it:
    // this is a frozen fact, and `FOR SHARE` would not conflict with the
    // `FOR SHARE` `finalize_payroll_run` holds while it works.
    let employment: Option<(String, bool)> =
        sqlx::query_as("SELECT employer_id, is_void FROM employment WHERE id = $1 FOR UPDATE")
            .bind(employment_id.as_str())
            .fetch_optional(&mut *tx)
            .await?;
    let (employer_id, is_void) =
        employment.ok_or_else(|| PayrollAppError::EmploymentNotFound(employment_id.clone()))?;
    if is_void {
        return Err(PayrollAppError::EmploymentIsVoid(employment_id.clone()));
    }
    let employer_id = EmployerId::new(employer_id);

    // ADR-0013: see the identical guard in `record_opening_balance` —
    // `finalized_payroll` never loses a row, so this counts a reversed
    // FinalizedPayroll exactly as a live one.
    if employment_has_a_finalization_in(&mut tx, employment_id, tax_year).await? {
        return Err(PayrollAppError::PriorEmploymentFrozenByFinalization {
            employment_id: employment_id.clone(),
            tax_year,
        });
    }

    // `xmax = 0` is true only for the row version this statement itself
    // inserted; an update leaves the prior version's `xmax` set. That is
    // what distinguishes a first declaration from a changed one, in the
    // same round trip as the write.
    let inserted: bool = sqlx::query_scalar(
        "INSERT INTO prior_employment_declaration
            (employment_id, tax_year, status, taxable_remuneration, paye, declared_by)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (employment_id, tax_year) DO UPDATE
         SET status = EXCLUDED.status,
             taxable_remuneration = EXCLUDED.taxable_remuneration,
             paye = EXCLUDED.paye,
             declared_at = now(),
             declared_by = EXCLUDED.declared_by
         RETURNING (xmax = 0)",
    )
    .bind(employment_id.as_str())
    .bind(tax_year.starting_year())
    .bind(status)
    .bind(taxable_remuneration)
    .bind(paye)
    .bind(declared_by)
    .fetch_one(&mut *tx)
    .await?;

    write_action_log_entry(
        &mut tx,
        ActionLogEntry {
            employer_id: &employer_id,
            actor: declared_by,
            action_type: if inserted {
                ActionType::PriorEmploymentDeclared
            } else {
                ActionType::PriorEmploymentChanged
            },
            target_type: "employment",
            target_id: employment_id.as_str(),
            context: None,
        },
    )
    .await?;

    tx.commit().await?;
    Ok(())
}

/// Reads back the `PriorEmployment` fact for one (Employment, TaxYear).
///
/// **No row means `Unknown`, and nothing else means `Unknown`** (§4.5b).
/// Returning the pure crate's three-valued type rather than an
/// `Option<..>` is what states that: an `Option` would give a caller a
/// second way to spell the same silence, and two spellings of silence is
/// how one of them eventually gets read as a confirmed none.
///
/// A missing Employment is not silence — it is a caller naming something
/// that does not exist — so it is refused rather than answered `Unknown`.
/// A void Employment is refused for §4.3's reason: this is a calculation
/// input, and a voided Employment reaches no payroll.
///
/// Public entry point over the opaque [`SaltDatabase`] handle. The
/// generic connection-taking implementation is `get_prior_employment_on`,
/// used internally by `year_to_date.rs`, which already holds a connection
/// or transaction and needs this read on that same one.
pub async fn get_prior_employment(
    db: &SaltDatabase,
    employment_id: &EmploymentId,
    tax_year: TaxYear,
) -> Result<PriorEmployment, PayrollAppError> {
    get_prior_employment_on(db.pool(), employment_id, tax_year).await
}

/// Takes anything a connection can be acquired from — a `&PgPool` for a
/// standalone read, or a `&mut Transaction` so a caller assembling several
/// facts at once reads them all on the one connection, inside its own
/// transaction and under whatever lock it already holds.
pub(crate) async fn get_prior_employment_on<'a>(
    conn: impl Acquire<'a, Database = Postgres>,
    employment_id: &EmploymentId,
    tax_year: TaxYear,
) -> Result<PriorEmployment, PayrollAppError> {
    let mut conn = conn.acquire().await?;

    type DeclarationRow = (bool, Option<String>, Option<i64>, Option<i64>);

    // One statement, and an outer join rather than two reads: "the
    // Employment exists" and "it has no declaration" are the two answers
    // this function must tell apart, and reading them separately would let
    // a void committing in between report the second.
    let row: Option<DeclarationRow> = sqlx::query_as(
        "SELECT employment.is_void,
                declaration.status,
                declaration.taxable_remuneration,
                declaration.paye
         FROM employment
         LEFT JOIN prior_employment_declaration AS declaration
                ON declaration.employment_id = employment.id
               AND declaration.tax_year = $2
         WHERE employment.id = $1",
    )
    .bind(employment_id.as_str())
    .bind(tax_year.starting_year())
    .fetch_optional(&mut *conn)
    .await?;

    let (is_void, status, taxable_remuneration, paye) =
        row.ok_or_else(|| PayrollAppError::EmploymentNotFound(employment_id.clone()))?;
    if is_void {
        return Err(PayrollAppError::EmploymentIsVoid(employment_id.clone()));
    }

    let Some(status) = status else {
        return Ok(PriorEmployment::Unknown);
    };

    let money = |cents: Option<i64>| {
        Money::from_cents(
            cents.expect("prior_employment_declaration CHECK: a 'present' row sets both figures"),
        )
        .expect("prior_employment_declaration CHECK: neither figure is negative, so it is a Money")
    };

    Ok(match status.as_str() {
        "confirmed_none" => PriorEmployment::None,
        "present" => PriorEmployment::Some(PriorEmploymentFigures::new(
            money(taxable_remuneration),
            money(paye),
        )),
        other => unreachable!(
            "prior_employment_declaration.status CHECK admits only 'confirmed_none' and 'present', not {other}"
        ),
    })
}
