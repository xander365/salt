//! `CreateEmployment`, `VoidEmployment`, and reading an Employment back as
//! the `EmploymentSnapshot` the pure crate accepts (§4.0, §4.3, §12), plus
//! the two Employer-scoped read models issue #51 adds.

use chrono::NaiveDate;
use payroll::{
    CompensationTerms, EmployerId, EmploymentId, EmploymentSnapshot, Money, PersonId,
    PersonReference,
};
use sqlx::{Acquire, Postgres};

use crate::action_log::{ActionLogEntry, ActionType, write_action_log_entry};
use crate::database::SaltDatabase;
use crate::error::PayrollAppError;
use crate::ids::new_id;

/// Names the Person a new Employment belongs to (issue #51, ADR-0020).
/// `salt-server`'s handler is what turns a request's "`personId` or
/// `fullName`, exactly one" into this — this type only ever holds one, so a
/// caller here can never smuggle in both or neither.
pub enum EmploymentPerson {
    /// An existing Person, verified to belong the same Employer inside the
    /// same transaction as the Employment insert. A `PersonId` belonging to
    /// another Employer, or naming nothing at all, refuses as
    /// [`PayrollAppError::PersonNotFound`] — the two are indistinguishable
    /// (ADR-0017).
    Existing(PersonId),
    /// A full name for a brand-new Person, created in the same transaction
    /// as the Employment. Two calls plus a client-side join would let a
    /// failure between them abandon a Person with no Employment, a row
    /// nobody would ever clean up — one transaction is what rules that out.
    New(String),
}

/// Records a new Employment against an Employer, naming the Person it is
/// for as either an existing Person or a full name for a brand-new one
/// (issue #51). Person and Employment are written in one transaction: a
/// failure — the Employer does not exist, the named Person belongs to
/// another Employer, the dates disagree — leaves neither row.
///
/// The Employer's existence is checked once, inside the transaction, ahead
/// of either insert: nothing ever deletes an `employer` row, so a row
/// confirmed present here cannot vanish before the transaction commits.
pub async fn create_employment(
    db: &SaltDatabase,
    employer_id: &EmployerId,
    person: EmploymentPerson,
    start_date: NaiveDate,
    end_date: Option<NaiveDate>,
    created_by: &str,
) -> Result<(PersonId, EmploymentId), PayrollAppError> {
    if let Some(end_date) = end_date
        && end_date < start_date
    {
        return Err(PayrollAppError::EmploymentEndsBeforeItStarts {
            start_date,
            end_date,
        });
    }

    let mut tx = db.pool().begin().await?;

    let employer_exists: Option<bool> =
        sqlx::query_scalar("SELECT TRUE FROM employer WHERE id = $1")
            .bind(employer_id.as_str())
            .fetch_optional(&mut *tx)
            .await?;
    if employer_exists.is_none() {
        return Err(PayrollAppError::EmployerNotFound(employer_id.clone()));
    }

    let person_id = match person {
        EmploymentPerson::New(full_name) => {
            if full_name.trim().is_empty() {
                return Err(PayrollAppError::PersonFullNameCannotBeEmpty);
            }
            let id = PersonId::new(new_id());
            sqlx::query(
                "INSERT INTO person (id, employer_id, full_name, created_by)
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(id.as_str())
            .bind(employer_id.as_str())
            .bind(&full_name)
            .bind(created_by)
            .execute(&mut *tx)
            .await?;
            id
        }
        EmploymentPerson::Existing(person_id) => {
            let belongs: Option<bool> =
                sqlx::query_scalar("SELECT TRUE FROM person WHERE id = $1 AND employer_id = $2")
                    .bind(person_id.as_str())
                    .bind(employer_id.as_str())
                    .fetch_optional(&mut *tx)
                    .await?;
            if belongs.is_none() {
                return Err(PayrollAppError::PersonNotFound(person_id));
            }
            person_id
        }
    };

    let employment_id = EmploymentId::new(new_id());
    sqlx::query(
        "INSERT INTO employment (id, employer_id, person_id, start_date, end_date, created_by)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(employment_id.as_str())
    .bind(employer_id.as_str())
    .bind(person_id.as_str())
    .bind(start_date)
    .bind(end_date)
    .bind(created_by)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok((person_id, employment_id))
}

/// One Employment in `GET /api/employers/{e}/employments` (issue #51): the
/// Employment's own id beside the Person's, and the Person's `fullName` —
/// every screen names a human, never an opaque id alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmploymentListing {
    pub id: EmploymentId,
    pub person_id: PersonId,
    pub full_name: String,
}

/// Every Employment `employer_id` has, named alongside its Person's
/// `fullName`. Filters on `employer_id` in SQL (ADR-0017's second layer);
/// no pagination, no filters, no sorting (issue #51's own Deep
/// Instructions) beyond the stable order below.
///
/// Ordered by `created_at, id` for the same reason
/// [`crate::list_employers_for_operator`] is: a stable, total order with no
/// sort parameter to expose.
pub async fn list_employments_for_employer(
    db: &SaltDatabase,
    employer_id: &EmployerId,
) -> Result<Vec<EmploymentListing>, PayrollAppError> {
    type Row = (String, String, String);

    let rows: Vec<Row> = sqlx::query_as(
        "SELECT employment.id, employment.person_id, person.full_name
         FROM employment
         JOIN person ON person.id = employment.person_id
         WHERE employment.employer_id = $1
         ORDER BY employment.created_at, employment.id",
    )
    .bind(employer_id.as_str())
    .fetch_all(db.pool())
    .await?;

    Ok(rows
        .into_iter()
        .map(|(id, person_id, full_name)| EmploymentListing {
            id: EmploymentId::new(id),
            person_id: PersonId::new(person_id),
            full_name,
        })
        .collect())
}

/// One Employment in `GET /api/employers/{e}/employments/{em}` (issue #51):
/// its dates, its Person's `fullName`, and its current pay — the
/// `CompensationTerms` row in force `as_of`, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmploymentDetail {
    pub id: EmploymentId,
    pub person_id: PersonId,
    pub full_name: String,
    pub start_date: NaiveDate,
    pub end_date: Option<NaiveDate>,
    pub current_basic_pay: Option<Money>,
}

/// Reads one Employment back for display, scoped to `employer_id` in SQL
/// (ADR-0017): an id belonging to another Employer is refused exactly like
/// one that does not exist at all, both as [`PayrollAppError::EmploymentNotFound`].
///
/// Unlike [`get_employment_snapshot`], a void Employment and one with no
/// `CompensationTerms` row in force are not refused — this is a display
/// read, not a calculation input, so it shows what there is to show rather
/// than demanding the Employment be calculation-ready.
pub async fn get_employment_detail(
    db: &SaltDatabase,
    employer_id: &EmployerId,
    employment_id: &EmploymentId,
    as_of: NaiveDate,
) -> Result<EmploymentDetail, PayrollAppError> {
    type Row = (String, String, NaiveDate, Option<NaiveDate>, Option<i64>);

    let row: Option<Row> = sqlx::query_as(
        "SELECT person.full_name, employment.person_id, employment.start_date,
                employment.end_date, in_force.basic_pay
         FROM employment
         JOIN person ON person.id = employment.person_id
         LEFT JOIN LATERAL (
             SELECT basic_pay
             FROM compensation_terms
             WHERE employment_id = employment.id AND effective_from <= $3
             ORDER BY effective_from DESC
             LIMIT 1
         ) AS in_force ON TRUE
         WHERE employment.id = $1 AND employment.employer_id = $2",
    )
    .bind(employment_id.as_str())
    .bind(employer_id.as_str())
    .bind(as_of)
    .fetch_optional(db.pool())
    .await?;

    let (full_name, person_id, start_date, end_date, basic_pay) =
        row.ok_or_else(|| PayrollAppError::EmploymentNotFound(employment_id.clone()))?;

    Ok(EmploymentDetail {
        id: employment_id.clone(),
        person_id: PersonId::new(person_id),
        full_name,
        start_date,
        end_date,
        current_basic_pay: basic_pay.map(|cents| {
            Money::from_cents(cents).expect(
                "compensation_terms.basic_pay CHECK: the column holds no negative amount, so it is a Money",
            )
        }),
    })
}

/// Marks an Employment void: never a physical delete (§4.3 — "delete when
/// nothing references it" is a racing check against a moving target). Writes
/// the `EmploymentVoided` ActionLog entry in the same transaction as the
/// void itself, so a committed void with no audit entry is not a reachable
/// state (§10).
///
/// Voiding an already-void Employment is refused rather than repeated. The
/// void has already happened, and a second `EmploymentVoided` entry would
/// record an act that did not — in a log no role may afterwards correct.
pub async fn void_employment(
    db: &SaltDatabase,
    employment_id: &EmploymentId,
    actor: &str,
) -> Result<(), PayrollAppError> {
    let mut tx = db.pool().begin().await?;

    // The `is_void = FALSE` predicate is what makes two concurrent voids
    // resolve to one: the second blocks on the first's row lock, then
    // matches nothing.
    let employer_id: Option<String> = sqlx::query_scalar(
        "UPDATE employment SET is_void = TRUE
         WHERE id = $1 AND is_void = FALSE
         RETURNING employer_id",
    )
    .bind(employment_id.as_str())
    .fetch_optional(&mut *tx)
    .await?;

    let Some(employer_id) = employer_id else {
        let exists: Option<bool> =
            sqlx::query_scalar("SELECT is_void FROM employment WHERE id = $1")
                .bind(employment_id.as_str())
                .fetch_optional(&mut *tx)
                .await?;
        return Err(match exists {
            Some(_) => PayrollAppError::EmploymentIsVoid(employment_id.clone()),
            None => PayrollAppError::EmploymentNotFound(employment_id.clone()),
        });
    };
    let employer_id = EmployerId::new(employer_id);

    write_action_log_entry(
        &mut tx,
        ActionLogEntry {
            employer_id: &employer_id,
            actor,
            action_type: ActionType::EmploymentVoided,
            target_type: "employment",
            target_id: employment_id.as_str(),
            context: None,
        },
    )
    .await?;

    tx.commit().await?;
    Ok(())
}

/// Reads an Employment back as an `EmploymentSnapshot`, together with
/// whichever `CompensationTerms` row is in force `as_of` that date: the one
/// with the latest `effective_from` on or before it. Its `effective_until`
/// is derived, never stored (§4.4) — the day before the next row's
/// `effective_from`, or `None` when no later row exists yet.
///
/// One statement, not three. The row in force and the row that ends it are
/// two halves of one fact, so reading them separately would let a
/// `CompensationTerms` row written in between produce a snapshot whose
/// `effective_until` never described the row it was attached to.
///
/// A void Employment is refused: this snapshot is a calculation input, and
/// §4.3 keeps a voided Employment out of every payroll.
///
/// Public entry point over the opaque [`SaltDatabase`] handle. The
/// generic connection-taking implementation is `get_employment_snapshot_on`,
/// used internally by callers — `calculate.rs` among them — that already
/// hold a transaction and need this read on that same connection.
pub async fn get_employment_snapshot(
    db: &SaltDatabase,
    employment_id: &EmploymentId,
    as_of: NaiveDate,
) -> Result<EmploymentSnapshot, PayrollAppError> {
    get_employment_snapshot_on(db.pool(), employment_id, as_of).await
}

/// Takes anything a connection can be acquired from — a `&PgPool` for a
/// standalone read, or a `&mut Transaction` so a caller assembling several
/// facts at once reads them all on the one connection, inside its own
/// transaction and under whatever lock it already holds.
pub(crate) async fn get_employment_snapshot_on<'a>(
    conn: impl Acquire<'a, Database = Postgres>,
    employment_id: &EmploymentId,
    as_of: NaiveDate,
) -> Result<EmploymentSnapshot, PayrollAppError> {
    let mut conn = conn.acquire().await?;

    type SnapshotRow = (
        String,
        String,
        NaiveDate,
        Option<NaiveDate>,
        bool,
        Option<NaiveDate>,
        Option<i64>,
        Option<NaiveDate>,
    );

    let row: Option<SnapshotRow> = sqlx::query_as(
        "SELECT employment.employer_id,
                employment.person_id,
                employment.start_date,
                employment.end_date,
                employment.is_void,
                in_force.effective_from,
                in_force.basic_pay,
                (SELECT MIN(later.effective_from)
                 FROM compensation_terms later
                 WHERE later.employment_id = employment.id
                   AND later.effective_from > in_force.effective_from)
         FROM employment
         LEFT JOIN LATERAL (
             SELECT effective_from, basic_pay
             FROM compensation_terms
             WHERE employment_id = employment.id AND effective_from <= $2
             ORDER BY effective_from DESC
             LIMIT 1
         ) AS in_force ON TRUE
         WHERE employment.id = $1",
    )
    .bind(employment_id.as_str())
    .bind(as_of)
    .fetch_optional(&mut *conn)
    .await?;

    let (employer_id, person_id, start_date, end_date, is_void, effective_from, basic_pay, next) =
        row.ok_or_else(|| PayrollAppError::EmploymentNotFound(employment_id.clone()))?;

    if is_void {
        return Err(PayrollAppError::EmploymentIsVoid(employment_id.clone()));
    }

    let (effective_from, basic_pay_cents) = effective_from
        .zip(basic_pay)
        .ok_or_else(|| PayrollAppError::NoCompensationTermsInForce(employment_id.clone()))?;

    let effective_until = next.map(|next| {
        next.pred_opt()
            .expect("a later effective_from is never the earliest representable date")
    });

    let basic_pay = Money::from_cents(basic_pay_cents).expect(
        "compensation_terms.basic_pay CHECK: the column holds no negative amount, so it is a Money",
    );
    let compensation_terms = CompensationTerms::new(effective_from, effective_until, basic_pay)
        .expect("effective_until, when present, is a later effective_from minus one day");

    Ok(EmploymentSnapshot::new(
        employment_id.clone(),
        EmployerId::new(employer_id),
        PersonReference::new(PersonId::new(person_id)),
        start_date,
        end_date,
        compensation_terms,
    ))
}
