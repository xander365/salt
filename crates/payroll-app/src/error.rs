use chrono::NaiveDate;
use payroll::{
    EmployerId, EmploymentId, PayPeriod, PayrollCalculation, PayrollError, PayrollInput,
    PayrollRules, PersonId, TaxYear,
};

use crate::finalize::FinalizedPayrollId;
use crate::operator::OperatorId;
use crate::payroll_run::PayrollRunId;
use crate::standing_pay_item::StandingPayItemId;

/// `payroll-app`'s own error type. It wraps [`PayrollError`] rather than
/// re-exporting it, because "PostgreSQL unavailable" and "run already
/// finalized" are different categories and never share an enum (ADR-0009).
///
/// Wrapping keeps the pure crate's refusals distinguishable by variant
/// while leaving room for the infrastructure and run-state variants this
/// crate will grow. `?` converts a [`PayrollError`] into the [`Payroll`]
/// variant; nothing else does.
///
/// [`Payroll`]: PayrollAppError::Payroll
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PayrollAppError {
    /// The pure calculator refused. The refusal is the whole error — this
    /// layer has nothing to add to it.
    Payroll(PayrollError),
    /// PostgreSQL refused the statement, or the connection failed. Never a
    /// domain refusal: a constraint violation surfaces here, not as a
    /// [`Payroll`] variant, even when it happens to guard a rule Rust also
    /// knows about.
    ///
    /// [`Payroll`]: PayrollAppError::Payroll
    Database(String),
    /// `SaltDatabase::connect` found the database's applied migration
    /// version behind the version compiled into this build. A typed
    /// refusal rather than letting the mismatch surface later as a raw
    /// database error the first time a use case reaches a column or table
    /// this build expects and an unmigrated database does not have.
    SchemaOutOfDate {
        /// The highest migration version compiled into this build.
        compiled: i64,
        /// The highest migration version applied to the database, or
        /// `None` when no migration has ever been applied to it.
        applied: Option<i64>,
    },
    /// `CreateEmployer` was given a name that is empty or only whitespace. A
    /// blank name shows a person nothing, the same demand every mandatory
    /// reason and attribution column in this crate makes of its own text
    /// (issue #40).
    EmployerNameCannotBeEmpty,
    /// No Employer exists with this id.
    EmployerNotFound(EmployerId),
    /// No Employment exists with this id.
    EmploymentNotFound(EmploymentId),
    /// `CreateEmployment` was given a `personId` naming no Person of this
    /// Employer — either the id names nothing at all, or it names a Person
    /// scoped to a different Employer (ADR-0020). The two are not
    /// distinguished: telling them apart would let a caller use this route
    /// to discover another Employer's Person ids (ADR-0017).
    PersonNotFound(PersonId),
    /// `CreateEmployment` was given a `fullName` that is empty or only
    /// whitespace. A blank name shows a person nothing, the same demand
    /// [`Self::EmployerNameCannotBeEmpty`] makes of its own text.
    PersonFullNameCannotBeEmpty,
    /// The Employment is void (§4.3). A voided Employment is a recorded
    /// mistake: it never appears in run membership, so no standing fact may
    /// be recorded against it and no calculation input may be read from it.
    EmploymentIsVoid(EmploymentId),
    /// The Employment's `end_date` falls before its `start_date`. A domain
    /// refusal, stated here rather than left to the table's own CHECK, so a
    /// caller is told the dates disagree instead of being handed a database
    /// error.
    EmploymentEndsBeforeItStarts {
        start_date: NaiveDate,
        end_date: NaiveDate,
    },
    /// No `CompensationTerms` row is in force for this Employment as of the
    /// requested date.
    NoCompensationTermsInForce(EmploymentId),
    /// A `PriorEmployment` declaration was given as `Unknown`. `Unknown` is
    /// what an absent row means (§4.5b) — it describes silence, not a fact
    /// to write, so declaring it explicitly is refused rather than stored.
    PriorEmploymentDeclarationCannotBeUnknown,
    /// An `UnsupportedDeductionStatus` declaration was given as `Unknown`,
    /// for the same reason: `Unknown` is what no row in force means (§4.5c),
    /// not a fact a declaration can state.
    UnsupportedDeductionDeclarationCannotBeUnknown,
    /// An `OpeningBalance`'s `SaltCoverageStart` is not a `PayPeriod` end
    /// date the Employer's `PaySchedule` generates (§4.5 guard 1).
    SaltCoverageStartNotAPeriodEnd { salt_coverage_start: NaiveDate },
    /// An `OpeningBalance`'s `SaltCoverageStart` does not fall inside the
    /// row's own `TaxYear` (§4.5 guard 2).
    SaltCoverageStartOutsideTaxYear {
        salt_coverage_start: NaiveDate,
        tax_year: TaxYear,
    },
    /// An `OpeningBalance`'s `SaltCoverageStart` falls before the
    /// Employment's first payable `PayPeriod` end in that `TaxYear` (§4.5
    /// guard 3): Salt cannot claim to have replaced a system for periods in
    /// which the Employment did not exist.
    SaltCoverageStartBeforeEmploymentIsPayable {
        salt_coverage_start: NaiveDate,
        first_payable_period_end: NaiveDate,
    },
    /// Non-zero `OpeningBalance` figures were given over an empty covered
    /// span — `SaltCoverageStart` equal to the Employment's first payable
    /// period end, so there is no pre-Salt period left for the figures to
    /// describe (§4.5 guard 4).
    OpeningBalanceFiguresOverAnEmptyCoveredSpan { salt_coverage_start: NaiveDate },
    /// A PayrollRun was asked for a `PayPeriod` the Employer's own
    /// `PaySchedule` does not generate (§4.2, §4.6). Everything downstream
    /// reads a run's period as one of the schedule's twelve: sequencing
    /// walks back to "the immediately preceding PayPeriod" (§8), the
    /// Ordinary uniqueness index keys on the period end alone (§4.6), and
    /// `calculate` resolves the CompensationTerms in force from the
    /// period's own boundaries. A period the schedule never generates has
    /// no predecessor and no successor, so it is refused at the one point
    /// it enters the system.
    ///
    /// `schedules_period` is the period the schedule does generate around
    /// the requested end date — the one the caller almost certainly meant,
    /// named for the same reason [`PayrollError::EffectiveFromNotAPeriodStart`]
    /// names the next valid date.
    PayPeriodNotGeneratedByThePaySchedule {
        period: PayPeriod,
        schedules_period: PayPeriod,
    },
    /// No PayrollRun exists with this id.
    PayrollRunNotFound(PayrollRunId),
    /// Removing a member is the deliberate omission path of an Ordinary run;
    /// Correction runs have the opposite membership semantics (§4.8).
    PayrollRunIsNotOrdinary(PayrollRunId),
    /// `RemoveEmploymentFromRun` was given a reason that is empty or only
    /// whitespace. Silent omission is the dangerous failure (§4.8) — a
    /// removal is a deliberate, reasoned act, and a blank reason states
    /// nothing while looking like it states something.
    RemovalReasonCannotBeEmpty,
    /// `payroll_run_id` and `employment_id` do not name a live membership:
    /// either the Employment was never proposed into that run, or it was
    /// already removed. The two are not distinguished, for the same reason
    /// `void_employment` does not distinguish "never existed" from
    /// "already void" any further than it needs to — either way there is no
    /// live membership to act on.
    ///
    /// Both `RemoveEmploymentFromRun` and `SetRunPayLines` refuse with it.
    /// Earning lines are a fact about paying this Employment for this
    /// period, so a run that is not paying it has nowhere to put them.
    EmploymentNotAnActiveRunMember {
        payroll_run_id: PayrollRunId,
        employment_id: EmploymentId,
    },
    /// `SetRunPayLines` was given a voluntary deduction of zero (issue #78,
    /// parent #70 §D-4). `Money` already refuses a negative or fractional
    /// amount; a zero one is refused here because it is not a line — it
    /// withholds nothing. Unlike a zero-amount allowance, which an Employer
    /// may deliberately state, a zero deduction only puts a line on a payslip
    /// that says nothing happened. `index` is the line's position in the
    /// `deductions` the caller sent.
    VoluntaryDeductionAmountIsZero { index: usize },
    /// Working state — membership, Earnings, the working calculation — was
    /// asked to change on a run that is already `Finalized`. Once history
    /// has been written there is nothing left to overwrite (§4.7). A
    /// `Calculated` run is not refused: editing it reopens it as `Draft`.
    PayrollRunAlreadyFinalized {
        payroll_run_id: PayrollRunId,
        /// Every `FinalizedPayroll` this run produced, paired with the
        /// Employment it belongs to and ordered by that Employment's id
        /// (issue #50, §0.28). A retry after a lost response is answered
        /// with the whole result, not a summary of it: an Ordinary run
        /// finalizes each active member into its own separate row, so a
        /// single id would be a guess for any run with more than one, and a
        /// caller that got no id at all could not show the success that
        /// already happened. Empty only for a vacuous run — one finalized
        /// with no active member.
        finalized_payrolls: Vec<(EmploymentId, FinalizedPayrollId)>,
    },
    /// `FinalizePayrollRun` was asked for a run that is not yet `Calculated`
    /// — still `Draft`, with at least one member unresolved. `Finalized` is
    /// the separate, absolute refusal above.
    PayrollRunNotCalculated(PayrollRunId),
    /// Finalization's rebuild of one member refused before any of the three
    /// comparisons could be made — an Employment voided, a
    /// `CompensationTerms` row withdrawn, a declaration removed since the
    /// run reached `Calculated`, or any refusal `calculate` itself raises.
    ///
    /// The Employment is named alongside the refusal for the same reason
    /// [`crate::PayrollRunCalculationRefusal`] names it and the three
    /// mismatches below do: an Employer told only "PriorEmployment is
    /// Unknown" about a ten-member run has been told nothing they can act
    /// on. The whole run refuses regardless (§5.1) — this says which member
    /// to go and fix.
    FinalizationRebuildRefused {
        employment_id: EmploymentId,
        refusal: Box<PayrollAppError>,
    },
    /// Finalization's reassembled `PayrollInput` no longer equals what the
    /// working calculation approved (§5.2). Carries both so the mismatch is
    /// explainable rather than merely declared — the whole point of
    /// comparing all three rather than the `PayrollCalculation` alone.
    FinalizationInputMismatch {
        employment_id: EmploymentId,
        approved: Box<PayrollInput>,
        current: Box<PayrollInput>,
    },
    /// Finalization's re-resolved `PayrollRules` no longer equal what the
    /// working calculation approved (§5.2) — e.g. a PAYE band corrected
    /// outside the range this Employee reaches, which leaves the money
    /// identical while the frozen rules would differ.
    FinalizationRulesMismatch {
        employment_id: EmploymentId,
        approved: Box<PayrollRules>,
        current: Box<PayrollRules>,
    },
    /// Finalization's recomputed `PayrollCalculation` no longer equals what
    /// the working calculation approved (§5.2).
    FinalizationCalculationMismatch {
        employment_id: EmploymentId,
        approved: Box<PayrollCalculation>,
        current: Box<PayrollCalculation>,
    },
    /// No `FinalizedPayroll` exists with this id.
    FinalizedPayrollNotFound(FinalizedPayrollId),
    /// A `FinalizedPayroll` snapshot names a layout this build does not know
    /// how to render. Finalized history is never migrated in place
    /// (ADR-0012), so readers must branch on the row's schema version rather
    /// than deserializing an unknown layout as if it were current.
    FinalizedPayrollSnapshotUnreadable {
        finalized_payroll_id: FinalizedPayrollId,
        schema_version: i32,
    },
    /// `ReverseFinalizedPayroll` was asked for a `FinalizedPayroll` that
    /// already has a `Reversal` (§6.1) — `reversal.finalized_payroll_id` is
    /// UNIQUE (migration 0011), and there is no unreversal record type to
    /// undo one with (ADR-0002's rejection, restated in §6.1).
    FinalizedPayrollAlreadyReversed(FinalizedPayrollId),
    /// `ReverseFinalizedPayroll` was given a reason that is empty or only
    /// whitespace. A reversal is a deliberate, attributed act (§6.1), the
    /// same demand `RemovalReasonCannotBeEmpty` makes of a run removal.
    ReversalReasonCannotBeEmpty,
    /// `RecordOpeningBalance` was asked to write or replace a row for an
    /// (Employment, TaxYear) that already has a `FinalizedPayroll` — Live or
    /// reversed (ADR-0013, §4.5). `OpeningBalance` is re-read into every
    /// later period's `YearToDateContext`, so an edit after that point would
    /// re-price already-finalized figures while their frozen snapshots kept
    /// showing the old ones.
    OpeningBalanceFrozenByFinalization {
        employment_id: EmploymentId,
        tax_year: TaxYear,
    },
    /// `DeclarePriorEmployment` was asked to write or replace a row for an
    /// (Employment, TaxYear) that already has a `FinalizedPayroll` — Live or
    /// reversed (ADR-0013, §4.5b). The same reason as
    /// [`Self::OpeningBalanceFrozenByFinalization`]: this fact is re-read into
    /// every later period's `YearToDateContext`.
    PriorEmploymentFrozenByFinalization {
        employment_id: EmploymentId,
        tax_year: TaxYear,
    },
    /// `ChangePaySchedule` was asked to change an Employer whose Employments
    /// already have a `FinalizedPayroll` — Live or reversed — in the given
    /// TaxYear (ADR-0013, §4.2). The guard that keeps a TaxYear at exactly
    /// twelve periods and cumulative PAYE sound.
    PayScheduleFrozenByFinalization {
        employer_id: EmployerId,
        tax_year: TaxYear,
    },
    /// `ChangePaySchedule` was asked to change an Employer who still has an
    /// unfinalized `PayrollRun` in the given TaxYear (§4.2). That run's
    /// period was cut by the schedule in force when it was created, and
    /// finalization re-derives everything from the current one (§5.1), so
    /// the change is refused here rather than surfacing later as a
    /// `FinalizationInputMismatch` nobody can act on.
    PayScheduleChangeBlockedByAnOpenRun {
        employer_id: EmployerId,
        period_end: NaiveDate,
    },
    /// `CreateOrdinaryPayrollRun` was asked for a run in a TaxYear holding a
    /// `FinalizedPayroll` whose period end the Employer's current
    /// `PaySchedule` does not generate (§4.2, ADR-0005) — the Employer's
    /// schedule moved inside a part-finalized TaxYear. The guard that holds
    /// "a TaxYear never contains other than twelve periods" without having
    /// to trust `change_pay_schedule`'s caller about what year it is.
    PayScheduleMovedWithinTaxYear {
        employer_id: EmployerId,
        tax_year: TaxYear,
        finalized_period_end: NaiveDate,
    },
    /// `ChangePaySchedule` was asked for a schedule that does not generate a
    /// boundary already stored against this Employer in the given TaxYear or
    /// a later one (§4.2, §4.4, §4.5, §4.5c).
    ///
    /// The `employer` table holds exactly one `PaySchedule` and no history,
    /// so every stored boundary is read under whichever schedule is current.
    /// Letting the change through would leave a `SaltCoverageStart` mid-period
    /// — the one thing INV-014 and §4.5 guard 1 exist to prevent — or a
    /// `CompensationTerms` row claiming a rise took effect on a date that is
    /// no longer the start of anything.
    ///
    /// Boundaries in earlier TaxYears are not checked, for the same reason
    /// the finalization guard is not: their periods have already been paid
    /// and their `PayrollInput` froze the schedule that cut them, so nothing
    /// re-reads them against today's schedule.
    PayScheduleChangeWouldStrandAStoredBoundary {
        employer_id: EmployerId,
        fact: ScheduleBoundedFact,
        boundary: NaiveDate,
    },
    /// A TaxYear whose own 1 March is not a representable `NaiveDate`, so the
    /// `PayPeriod` every reader locates from it (ADR-0005) cannot be derived.
    /// Refused rather than panicked on, and named as the TaxYear it is rather
    /// than as some substitute date, so the refusal points at what the caller
    /// actually supplied.
    TaxYearOutsideRepresentableCalendar { tax_year: TaxYear },
    /// `FinalizePayrollRun` was asked to finalize an Ordinary run whose
    /// immediately preceding `PayPeriod`, in the same `TaxYear`, is not
    /// resolved (§5.3 step 3, §7.1) for `employment_id` — no live
    /// `FinalizedPayroll`, no reasoned removal or reversal, no
    /// `OpeningBalance` boundary before it, and the Employment did overlap
    /// it. Converts what would otherwise be silent under-withholding into a
    /// visible refusal naming both the Employment and the unresolved
    /// `period`.
    PrecedingPeriodUnresolved {
        employment_id: EmploymentId,
        period: PayPeriod,
    },
    /// `CreateCorrectionRun` was given a reason that is empty or only
    /// whitespace — the same up-front demand `RemovalReasonCannotBeEmpty`
    /// and `ReversalReasonCannotBeEmpty` make of their own mandatory
    /// reasons (§4.6).
    CorrectionReasonCannotBeEmpty,
    /// `AddEmploymentToCorrectionRun` was given a run that is not a
    /// Correction run — the opposite of [`Self::PayrollRunIsNotOrdinary`]
    /// (§4.8).
    PayrollRunIsNotCorrection(PayrollRunId),
    /// `AddEmploymentToCorrectionRun` was asked to add a second Employment.
    /// A Correction run holds exactly one (§4.8, ADR-0015); the trigger
    /// from migration 0012 would refuse this too, but this is the named
    /// domain refusal a caller can match on rather than a raw database
    /// exception.
    CorrectionRunAlreadyHasAnEmployment(PayrollRunId),
    /// `AddEmploymentToCorrectionRun` was given an Employment that does not
    /// belong to the Correction run's own Employer.
    CorrectionEmploymentBelongsToADifferentEmployer {
        employment_id: EmploymentId,
        employer_id: EmployerId,
    },
    /// `AddEmploymentToCorrectionRun` was given a `replaces` target that
    /// does not name this Employment, this Employer and this run's own
    /// period end (§4.8).
    CorrectionTargetDoesNotMatch {
        payroll_run_id: PayrollRunId,
        finalized_payroll_id: FinalizedPayrollId,
    },
    /// `AddEmploymentToCorrectionRun` was given a `replaces` target with no
    /// `Reversal` — only a reversed `FinalizedPayroll` has anything for a
    /// replacement to replace (§4.8, §6.3).
    CorrectionTargetNotReversed(FinalizedPayrollId),
    /// `FinalizePayrollRun` lost the race the `replaces_finalized_payroll_id`
    /// UNIQUE constraint on `finalized_payroll` decides (§4.8, §9): another
    /// Correction run finalized against the same target first. Two draft
    /// Correction runs may legitimately name the same target; only the
    /// first to finalize wins.
    CorrectionTargetAlreadyReplaced(FinalizedPayrollId),
    /// `FinalizePayrollRun` was asked to finalize a Correction run whose
    /// single member names no target, and neither of §4.8's two
    /// null-lineage cases holds: the Employment was not removed with a
    /// reason from the finalized Ordinary run for this `period`, and it was
    /// a member of one.
    CorrectionLineageNotLegitimate {
        employment_id: EmploymentId,
        period: PayPeriod,
    },
    /// `FinalizePayrollRun` was asked to finalize a Correction run whose
    /// single member names no target, while an unreplaced reversed
    /// `FinalizedPayroll` for that Employment and period does exist. §4.8
    /// sets lineage *exactly* when a reversed predecessor exists, so a null
    /// target here would leave that predecessor unreplaced forever and fork
    /// the chain ADR-0015 keeps linear. The predecessor to name is carried.
    CorrectionLineageOmitsAReversedPredecessor {
        employment_id: EmploymentId,
        period: PayPeriod,
        finalized_payroll_id: FinalizedPayrollId,
    },
    /// `FinalizePayrollRun` was asked to write a `FinalizedPayroll` for an
    /// Employment and `PayPeriod` that already has a live one (§6.2).
    ///
    /// Reachable two ways, and both mean the same thing to a caller. A
    /// second null-lineage Correction run for a period a first one has
    /// already paid passes §4.8's two cases — the Ordinary run still
    /// accounts for the Employment's absence, and no reversed predecessor
    /// is unreplaced — and only the live record itself says the period has
    /// since been paid. And where two such runs finalize at the same
    /// moment, the `live_finalized_payroll` primary key is what decides
    /// between them (§5.4), exactly as the `replaces_finalized_payroll_id`
    /// UNIQUE constraint decides between two Corrections naming one target.
    ///
    /// The way forward is the same in both: reverse the live record and
    /// name it as this Correction's target, so the chain stays linear
    /// (ADR-0015). The record is not carried here because
    /// `(employment_id, period_end)` *is* the liveness primary key — one
    /// lookup names it, and a stale id would not.
    CorrectionPeriodAlreadyHasALivePayroll {
        employment_id: EmploymentId,
        period: PayPeriod,
    },
    /// A `CompensationTerms` write that diverges from live finalized payroll
    /// was given a reason that is empty or only whitespace — the same demand
    /// every other mandatory reason in this crate makes (§6.5 guard 1).
    ///
    /// `CorrectCompensationTerms` makes it up front, of every correction.
    /// `RecordCompensationTerms` makes it only once its divergence list comes
    /// back non-empty, because an insert that diverges from nothing is an
    /// ordinary pay rise and owes no explanation — and it makes it *after*
    /// the acknowledgement check, so a caller asking what diverges is told
    /// the list rather than asked for a sentence it has no reason to write.
    CompensationTermsCorrectionReasonCannotBeEmpty,
    /// `CorrectCompensationTerms` was asked to correct a `CompensationTerms`
    /// row that does not exist at the given `(employment_id, effective_from)`
    /// — the pair that identifies one, since the table exposes no id to a
    /// caller (`UNIQUE (employment_id, effective_from)`, migration 0003).
    NoCompensationTermsRowAt {
        employment_id: EmploymentId,
        effective_from: NaiveDate,
    },
    /// `DeclareUnsupportedDeductionStatus` was given a reason that is empty
    /// or only whitespace — the same up-front demand as
    /// [`Self::CompensationTermsCorrectionReasonCannotBeEmpty`]. Every write
    /// against this fact, first declaration or later change, is a
    /// correction §6.5 requires a reason for; the ActionType catalogue names
    /// only one act here (see `ActionType::UnsupportedDeductionStatusCorrected`).
    UnsupportedDeductionDeclarationReasonCannotBeEmpty,
    /// A master-data correction was asked for without acknowledging exactly
    /// the Live finalized `PayPeriod`s it now diverges from (§6.5 guard 2).
    ///
    /// This is not a refusal *of the correction*: divergence never blocks a
    /// correction, and re-asking with `diverging_periods` acknowledged
    /// carries the same correction through unchanged. It is the refusal that
    /// makes the acknowledgement real, because the list reaches the caller
    /// here — a list computed and dropped would leave the ActionLog
    /// recording an acknowledgement nobody was ever shown.
    ///
    /// `diverging_periods` is the list as it stands *now*, recomputed inside
    /// the correction's own transaction, so an acknowledgement of a list a
    /// concurrent write has since changed is refused rather than honoured.
    MasterDataDivergenceNotAcknowledged {
        employment_id: EmploymentId,
        diverging_periods: Vec<PayPeriod>,
    },
    /// A `CompensationTerms` write would put a row on a date where this
    /// Employment already has one — `CorrectCompensationTerms` moving one
    /// there, or `RecordCompensationTerms` inserting a second (issue #69).
    /// Stated as a domain refusal rather than left to the table's `UNIQUE
    /// (employment_id, effective_from)`, so a caller is told which date
    /// collided instead of being handed a database error for a rule Rust
    /// can name.
    CompensationTermsAlreadyExistAt {
        employment_id: EmploymentId,
        effective_from: NaiveDate,
    },
    /// `SetEmployerParticulars` was given a `registeredName` that is empty
    /// or only whitespace. The same demand [`Self::EmployerNameCannotBeEmpty`]
    /// makes of its own text.
    EmployerParticularsRegisteredNameCannotBeEmpty,
    /// `SetEmployerParticulars` was given an `addressLine1` that is empty or
    /// only whitespace.
    EmployerParticularsAddressLine1CannotBeEmpty,
    /// `SetEmployerParticulars` was given a `city` that is empty or only
    /// whitespace.
    EmployerParticularsCityCannotBeEmpty,
    /// A `SetEmployerParticulars` write that either corrects an existing row
    /// or diverges from live finalized payroll was given a reason that is
    /// empty or only whitespace — the same demand
    /// [`Self::CompensationTermsCorrectionReasonCannotBeEmpty`] makes of its
    /// own two callers (§6.5 guard 1). Correcting an existing row demands it
    /// unconditionally, the same as [`correct_compensation_terms`]; recording
    /// one for the first time demands it only once its divergence list comes
    /// back non-empty, the same as [`record_compensation_terms`] — an insert
    /// that diverges from nothing owes no explanation.
    ///
    /// [`correct_compensation_terms`]: crate::correct_compensation_terms
    /// [`record_compensation_terms`]: crate::record_compensation_terms
    EmployerParticularsCorrectionReasonCannotBeEmpty,
    /// A master-data write to `EmployerParticulars` was asked for without
    /// acknowledging exactly the Live finalized `PayPeriod`s it now diverges
    /// from — the Employer-scoped sibling of
    /// [`Self::MasterDataDivergenceNotAcknowledged`], for a fact that
    /// diverges from every Live finalized period this Employer has rather
    /// than a dated span of them (§6.5, issue #71).
    EmployerMasterDataDivergenceNotAcknowledged {
        employer_id: EmployerId,
        diverging_periods: Vec<PayPeriod>,
    },
    /// `SetPersonParticulars` was given an `identityNumber` that is empty or
    /// only whitespace.
    PersonParticularsIdentityNumberCannotBeEmpty,
    /// `SetPersonParticulars` was given an `addressLine1` that is empty or
    /// only whitespace.
    PersonParticularsAddressLine1CannotBeEmpty,
    /// `SetPersonParticulars` was given a `city` that is empty or only
    /// whitespace.
    PersonParticularsCityCannotBeEmpty,
    /// A `SetPersonParticulars` write that either corrects an existing row or
    /// diverges from live finalized payroll was given a reason that is empty
    /// or only whitespace — [`Self::EmployerParticularsCorrectionReasonCannotBeEmpty`]'s
    /// own demand, Person-scoped (issue #72).
    PersonParticularsCorrectionReasonCannotBeEmpty,
    /// `CorrectPersonFullName` was given a reason that is empty or only
    /// whitespace. Unlike [`Self::PersonParticularsCorrectionReasonCannotBeEmpty`],
    /// this is demanded unconditionally: a Person's `full_name` is set the
    /// moment the Person is created (issue #51), so every write this use
    /// case ever makes corrects an existing value — there is no "first
    /// record" case that could owe no explanation.
    PersonNameCorrectionReasonCannotBeEmpty,
    /// A master-data write to a Person's `PersonParticulars` or `full_name`
    /// was asked for without acknowledging exactly the Live finalized
    /// `PayPeriod`s it now diverges from — the Person-scoped sibling of
    /// [`Self::EmployerMasterDataDivergenceNotAcknowledged`], spanning every
    /// Employment this Person has rather than one Employer's (issue #72).
    PersonMasterDataDivergenceNotAcknowledged {
        person_id: PersonId,
        diverging_periods: Vec<PayPeriod>,
    },
    /// `CreateOperator` was given an email that is empty or only whitespace.
    OperatorEmailCannotBeEmpty,
    /// `CreateOperator` was given a display name that is empty or only
    /// whitespace.
    OperatorDisplayNameCannotBeEmpty,
    /// `CreateOperator` was given a password shorter than the minimum
    /// credential length required by the password-handling design.
    OperatorPasswordTooShort { minimum: usize },
    /// `CreateOperator` was given a password longer than the maximum
    /// credential length required by the password-handling design.
    OperatorPasswordTooLong { maximum: usize },
    /// `CreateOperator` was given an email that collides, case-insensitively,
    /// with an email already recorded for another Operator — the folded
    /// form the `operator_email_folded_key` UNIQUE index compares (issue
    /// #41).
    OperatorEmailAlreadyInUse,
    /// Argon2id hashing itself failed while creating an Operator — a
    /// hashing-library or RNG failure, never a fact about the password
    /// given. Distinct from [`Self::Database`] because it names a different
    /// dependency: PostgreSQL was never involved.
    PasswordHashingFailed(String),
    /// No Operator exists with this id.
    OperatorNotFound(OperatorId),
    /// `DisableOperator` was asked to disable an Operator already disabled.
    /// The disabling has already happened, so a second act would record one
    /// that did not.
    OperatorAlreadyDisabled(OperatorId),
    /// `VerifyOperatorCredential` refused. Deliberately the *only* refusal
    /// this use case ever raises, and it carries no data: an unknown email,
    /// a wrong password, and a disabled Operator all reach this one variant,
    /// because a caller — or an attacker — must not be able to tell the
    /// three apart (issue #41).
    OperatorCredentialInvalid,
    /// `CreateEmployerMembership` was asked to grant a pair that already has
    /// a membership row, active or revoked (issue #43: unique on the pair).
    EmployerMembershipAlreadyExists {
        operator_id: OperatorId,
        employer_id: EmployerId,
    },
    /// No EmployerMembership exists for this Operator and Employer.
    EmployerMembershipNotFound {
        operator_id: OperatorId,
        employer_id: EmployerId,
    },
    /// `RevokeEmployerMembership` was asked to revoke a membership already
    /// revoked. The revocation has already happened, so a second one would
    /// record an act that did not.
    EmployerMembershipAlreadyRevoked {
        operator_id: OperatorId,
        employer_id: EmployerId,
    },
    /// `Bootstrap` was called while an Operator already exists (issue #48,
    /// §0.2). Bootstrap is a first-run command, not an administrative back
    /// door: it is refused as a whole, before any of its three inserts,
    /// rather than gaining a `--force` flag or an "add another Operator"
    /// mode.
    BootstrapOperatorAlreadyExists,
    /// `Bootstrap` could not take its `LOCK TABLE operator IN EXCLUSIVE
    /// MODE` within the wait it allows itself, because something else is
    /// holding a conflicting lock on `operator` (issue #48). A database with
    /// no Operator in it has no such writer, so this is a running system, not
    /// the fresh one bootstrap is for. Refused rather than waited out: an
    /// `EXCLUSIVE` request that keeps queueing blocks every later reader of
    /// the table behind it, which is every sign-in on that server.
    BootstrapOperatorTableBusy,
    /// `Bootstrap` was given a `--period-end-day` outside 1..=28 and not the
    /// literal "last-day-of-month". The CLI layer only checks that the value
    /// parses at all; this range is a domain rule of
    /// [`payroll::DayOfMonth`], so it is enforced here rather than
    /// duplicated in `salt-server`'s argument parser.
    BootstrapPeriodEndDayInvalid { day: u8 },
    /// No `StandingPayItem` exists with this id.
    StandingPayItemNotFound(StandingPayItemId),
    /// `EndStandingPayItem` was asked to end an item already ended. The
    /// ending has already happened, so a second one would record an act
    /// that did not (the same reasoning `EmployerMembershipAlreadyRevoked`
    /// applies to a membership).
    StandingPayItemAlreadyEnded(StandingPayItemId),
    /// `EndStandingPayItem` was given a reason that is empty or only
    /// whitespace. Ending a `StandingPayItem` is a deliberate, attributed
    /// act (§0) — a historical proposal must always have something to
    /// point at, so a blank reason states nothing while looking like it
    /// states something.
    StandingPayItemEndReasonCannotBeEmpty,
    /// `CreateStandingPayItem` was given a medical aid premium of zero. It
    /// withholds nothing and is not a deduction (§D-4) — the same rule
    /// `VoluntaryDeductionAmountIsZero` applies to a line typed on a run,
    /// stated here without a line index because an item is not a list.
    StandingMedicalAidPremiumIsZero,
}

/// Which stored fact carries the boundary a `PaySchedule` change would
/// strand. Typed rather than free text, for the same reason `ActionType` is:
/// a caller deciding what to show the Employer next matches on a variant
/// instead of parsing a sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleBoundedFact {
    /// `opening_balance.first_salt_period_end`, which must be a `PayPeriod`
    /// **end** the schedule generates (§4.5 guard 1).
    SaltCoverageStart,
    /// `compensation_terms.effective_from`, which must be a `PayPeriod`
    /// **start** (INV-014).
    CompensationTermsEffectiveFrom,
    /// `unsupported_deduction_declaration.effective_from`, which must be a
    /// `PayPeriod` **start**, so exactly one row governs a period (§4.5c).
    UnsupportedDeductionEffectiveFrom,
}

impl std::fmt::Display for ScheduleBoundedFact {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::SaltCoverageStart => "an OpeningBalance's SaltCoverageStart",
            Self::CompensationTermsEffectiveFrom => "a CompensationTerms effective-from date",
            Self::UnsupportedDeductionEffectiveFrom => {
                "an UnsupportedDeductionStatus effective-from date"
            }
        };
        f.write_str(name)
    }
}

impl std::fmt::Display for PayrollAppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Payroll(err) => write!(f, "{err}"),
            Self::Database(message) => write!(f, "database error: {message}"),
            Self::SchemaOutOfDate { compiled, applied } => match applied {
                Some(applied) => write!(
                    f,
                    "the database's applied migration version {applied} is behind the version \
                     {compiled} compiled into this build"
                ),
                None => write!(
                    f,
                    "the database has no applied migrations, but this build compiled in \
                     migration version {compiled}"
                ),
            },
            Self::EmployerNameCannotBeEmpty => {
                write!(f, "an Employer name must not be empty")
            }
            Self::EmployerNotFound(id) => write!(f, "no Employer exists with id {id}"),
            Self::EmploymentNotFound(id) => write!(f, "no Employment exists with id {id}"),
            Self::PersonNotFound(id) => write!(f, "no Person exists with id {id}"),
            Self::PersonFullNameCannotBeEmpty => {
                write!(f, "a Person full name must not be empty")
            }
            Self::EmploymentIsVoid(id) => write!(f, "Employment {id} is void"),
            Self::EmploymentEndsBeforeItStarts {
                start_date,
                end_date,
            } => write!(
                f,
                "an Employment ending {end_date} cannot start later, on {start_date}"
            ),
            Self::NoCompensationTermsInForce(id) => {
                write!(f, "no CompensationTerms are in force for Employment {id}")
            }
            Self::PriorEmploymentDeclarationCannotBeUnknown => write!(
                f,
                "a PriorEmployment declaration cannot itself be Unknown; omit the declaration instead"
            ),
            Self::UnsupportedDeductionDeclarationCannotBeUnknown => write!(
                f,
                "an UnsupportedDeductionStatus declaration cannot itself be Unknown; omit the declaration instead"
            ),
            Self::SaltCoverageStartNotAPeriodEnd {
                salt_coverage_start,
            } => write!(
                f,
                "SaltCoverageStart {salt_coverage_start} is not a PayPeriod end date the Employer's PaySchedule generates"
            ),
            Self::SaltCoverageStartOutsideTaxYear {
                salt_coverage_start,
                tax_year,
            } => write!(
                f,
                "SaltCoverageStart {salt_coverage_start} falls outside TaxYear {}",
                tax_year.starting_year()
            ),
            Self::SaltCoverageStartBeforeEmploymentIsPayable {
                salt_coverage_start,
                first_payable_period_end,
            } => write!(
                f,
                "SaltCoverageStart {salt_coverage_start} falls before the Employment's first payable PayPeriod end {first_payable_period_end} in that TaxYear"
            ),
            Self::OpeningBalanceFiguresOverAnEmptyCoveredSpan {
                salt_coverage_start,
            } => write!(
                f,
                "non-zero OpeningBalance figures were given over an empty covered span: SaltCoverageStart {salt_coverage_start} is itself the Employment's first payable PayPeriod end, so no pre-Salt period remains for them to describe"
            ),
            Self::PayPeriodNotGeneratedByThePaySchedule {
                period,
                schedules_period,
            } => write!(
                f,
                "PayPeriod {} to {} is not one the Employer's PaySchedule generates; \
                 the schedule's own period around that end date is {} to {}",
                period.start(),
                period.end(),
                schedules_period.start(),
                schedules_period.end()
            ),
            Self::PayrollRunNotFound(id) => write!(f, "no PayrollRun exists with id {id}"),
            Self::PayrollRunIsNotOrdinary(id) => {
                write!(f, "PayrollRun {id} is not an Ordinary run")
            }
            Self::RemovalReasonCannotBeEmpty => {
                write!(f, "a removal reason must not be empty")
            }
            Self::EmploymentNotAnActiveRunMember {
                payroll_run_id,
                employment_id,
            } => write!(
                f,
                "Employment {employment_id} is not an active member of PayrollRun {payroll_run_id}"
            ),
            Self::VoluntaryDeductionAmountIsZero { index } => {
                write!(
                    f,
                    "deduction line {index} withholds nothing: its amount is zero"
                )
            }
            Self::PayrollRunAlreadyFinalized { payroll_run_id, .. } => {
                write!(f, "PayrollRun {payroll_run_id} is already Finalized")
            }
            Self::PayrollRunNotCalculated(id) => {
                write!(f, "PayrollRun {id} is not Calculated")
            }
            Self::FinalizationRebuildRefused {
                employment_id,
                refusal,
            } => write!(
                f,
                "finalizing Employment {employment_id} refused: {refusal}"
            ),
            Self::FinalizationInputMismatch {
                employment_id,
                approved,
                current,
            } => write!(
                f,
                "finalizing Employment {employment_id} refused: the reassembled PayrollInput no \
                 longer equals what was approved — {}",
                describe_differences(
                    serde_json::to_value(approved).ok(),
                    serde_json::to_value(current).ok(),
                )
            ),
            Self::FinalizationRulesMismatch {
                employment_id,
                approved,
                current,
            } => write!(
                f,
                "finalizing Employment {employment_id} refused: the re-resolved PayrollRules no \
                 longer equal what was approved — {}",
                describe_differences(
                    serde_json::to_value(approved).ok(),
                    serde_json::to_value(current).ok(),
                )
            ),
            Self::FinalizationCalculationMismatch {
                employment_id,
                approved,
                current,
            } => write!(
                f,
                "finalizing Employment {employment_id} refused: the recomputed \
                 PayrollCalculation no longer equals what was approved — {}",
                describe_differences(
                    serde_json::to_value(approved).ok(),
                    serde_json::to_value(current).ok(),
                )
            ),
            Self::FinalizedPayrollNotFound(id) => {
                write!(f, "no FinalizedPayroll exists with id {id}")
            }
            Self::FinalizedPayrollSnapshotUnreadable {
                finalized_payroll_id,
                schema_version,
            } => write!(
                f,
                "FinalizedPayroll {finalized_payroll_id} has snapshot schema version \
                 {schema_version}, which this build cannot read"
            ),
            Self::FinalizedPayrollAlreadyReversed(id) => {
                write!(f, "FinalizedPayroll {id} has already been reversed")
            }
            Self::ReversalReasonCannotBeEmpty => {
                write!(f, "a reversal reason must not be empty")
            }
            Self::OpeningBalanceFrozenByFinalization {
                employment_id,
                tax_year,
            } => write!(
                f,
                "OpeningBalance for Employment {employment_id} in TaxYear {} is frozen: \
                 that Employment has already finalized a payroll in that TaxYear",
                tax_year.starting_year()
            ),
            Self::PriorEmploymentFrozenByFinalization {
                employment_id,
                tax_year,
            } => write!(
                f,
                "the PriorEmployment declaration for Employment {employment_id} in TaxYear {} \
                 is frozen: that Employment has already finalized a payroll in that TaxYear",
                tax_year.starting_year()
            ),
            Self::PayScheduleFrozenByFinalization {
                employer_id,
                tax_year,
            } => write!(
                f,
                "Employer {employer_id}'s PaySchedule is frozen for TaxYear {}: a payroll has \
                 already been finalized in that TaxYear or a later one",
                tax_year.starting_year()
            ),
            Self::PayScheduleChangeBlockedByAnOpenRun {
                employer_id,
                period_end,
            } => write!(
                f,
                "Employer {employer_id}'s PaySchedule cannot change while the payroll run for \
                 the period ending {period_end} is not finalized"
            ),
            Self::PayScheduleMovedWithinTaxYear {
                employer_id,
                tax_year,
                finalized_period_end,
            } => write!(
                f,
                "Employer {employer_id}'s PaySchedule does not generate the period ending \
                 {finalized_period_end}, which is already finalized in TaxYear {}: the schedule \
                 moved inside a TaxYear that had already finalized payroll",
                tax_year.starting_year()
            ),
            Self::PayScheduleChangeWouldStrandAStoredBoundary {
                employer_id,
                fact,
                boundary,
            } => write!(
                f,
                "Employer {employer_id}'s new PaySchedule does not generate {boundary}, which is \
                 already recorded as {fact}: the change would leave that boundary mid-period"
            ),
            Self::TaxYearOutsideRepresentableCalendar { tax_year } => write!(
                f,
                "TaxYear {} has no representable 1 March, so its first PayPeriod cannot be \
                 derived",
                tax_year.starting_year()
            ),
            Self::PrecedingPeriodUnresolved {
                employment_id,
                period,
            } => write!(
                f,
                "finalizing Employment {employment_id} refused: the immediately preceding \
                 PayPeriod {} to {} is not resolved — no live FinalizedPayroll, no reasoned \
                 removal or reversal, and no OpeningBalance boundary covers it",
                period.start(),
                period.end()
            ),
            Self::CorrectionReasonCannotBeEmpty => {
                write!(f, "a correction reason must not be empty")
            }
            Self::PayrollRunIsNotCorrection(id) => {
                write!(f, "PayrollRun {id} is not a Correction run")
            }
            Self::CorrectionRunAlreadyHasAnEmployment(id) => write!(
                f,
                "Correction PayrollRun {id} already holds an Employment; a Correction run holds \
                 exactly one"
            ),
            Self::CorrectionEmploymentBelongsToADifferentEmployer {
                employment_id,
                employer_id,
            } => write!(
                f,
                "Employment {employment_id} does not belong to Employer {employer_id}"
            ),
            Self::CorrectionTargetDoesNotMatch {
                payroll_run_id,
                finalized_payroll_id,
            } => write!(
                f,
                "FinalizedPayroll {finalized_payroll_id} does not match the Employment, Employer \
                 and period of Correction PayrollRun {payroll_run_id}"
            ),
            Self::CorrectionTargetNotReversed(id) => write!(
                f,
                "FinalizedPayroll {id} has not been reversed, so there is nothing for a \
                 Correction to replace"
            ),
            Self::CorrectionTargetAlreadyReplaced(id) => write!(
                f,
                "FinalizedPayroll {id} has already been replaced by another Correction"
            ),
            Self::CorrectionLineageNotLegitimate {
                employment_id,
                period,
            } => write!(
                f,
                "finalizing Correction for Employment {employment_id} refused: no target was \
                 named, but the PayPeriod {} to {} was neither an omission (the Employment was \
                 never a member of the finalized Ordinary run for it) nor a reasoned removal \
                 from it",
                period.start(),
                period.end()
            ),
            Self::CorrectionLineageOmitsAReversedPredecessor {
                employment_id,
                period,
                finalized_payroll_id,
            } => write!(
                f,
                "finalizing Correction for Employment {employment_id} refused: no target was \
                 named, but reversed FinalizedPayroll {finalized_payroll_id} for the PayPeriod \
                 {} to {} is still unreplaced and must be named as the target",
                period.start(),
                period.end()
            ),
            Self::CorrectionPeriodAlreadyHasALivePayroll {
                employment_id,
                period,
            } => write!(
                f,
                "finalizing Correction for Employment {employment_id} refused: the PayPeriod {} \
                 to {} already has a live FinalizedPayroll, which must be reversed and named as \
                 this Correction's target",
                period.start(),
                period.end()
            ),
            Self::CompensationTermsCorrectionReasonCannotBeEmpty => {
                write!(f, "a CompensationTerms correction reason must not be empty")
            }
            Self::NoCompensationTermsRowAt {
                employment_id,
                effective_from,
            } => write!(
                f,
                "no CompensationTerms row exists for Employment {employment_id} effective \
                 {effective_from}"
            ),
            Self::UnsupportedDeductionDeclarationReasonCannotBeEmpty => write!(
                f,
                "an UnsupportedDeductionStatus declaration reason must not be empty"
            ),
            Self::MasterDataDivergenceNotAcknowledged {
                employment_id,
                diverging_periods,
            } => {
                write!(
                    f,
                    "correcting master data for Employment {employment_id} needs the \
                     acknowledgement of every Live finalized PayPeriod it now diverges from: "
                )?;
                if diverging_periods.is_empty() {
                    return write!(f, "none");
                }
                for (index, period) in diverging_periods.iter().enumerate() {
                    if index > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{} to {}", period.start(), period.end())?;
                }
                Ok(())
            }
            Self::CompensationTermsAlreadyExistAt {
                employment_id,
                effective_from,
            } => write!(
                f,
                "Employment {employment_id} already has a CompensationTerms row effective \
                 {effective_from}"
            ),
            Self::EmployerParticularsRegisteredNameCannotBeEmpty => {
                write!(
                    f,
                    "an EmployerParticulars registered name must not be empty"
                )
            }
            Self::EmployerParticularsAddressLine1CannotBeEmpty => {
                write!(f, "an EmployerParticulars address line 1 must not be empty")
            }
            Self::EmployerParticularsCityCannotBeEmpty => {
                write!(f, "an EmployerParticulars city must not be empty")
            }
            Self::EmployerParticularsCorrectionReasonCannotBeEmpty => {
                write!(
                    f,
                    "an EmployerParticulars correction reason must not be empty"
                )
            }
            Self::EmployerMasterDataDivergenceNotAcknowledged {
                employer_id,
                diverging_periods,
            } => {
                write!(
                    f,
                    "correcting EmployerParticulars for Employer {employer_id} needs the \
                     acknowledgement of every Live finalized PayPeriod it now diverges from: "
                )?;
                if diverging_periods.is_empty() {
                    return write!(f, "none");
                }
                for (index, period) in diverging_periods.iter().enumerate() {
                    if index > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{} to {}", period.start(), period.end())?;
                }
                Ok(())
            }
            Self::PersonParticularsIdentityNumberCannotBeEmpty => {
                write!(f, "a PersonParticulars identity number must not be empty")
            }
            Self::PersonParticularsAddressLine1CannotBeEmpty => {
                write!(f, "a PersonParticulars address line 1 must not be empty")
            }
            Self::PersonParticularsCityCannotBeEmpty => {
                write!(f, "a PersonParticulars city must not be empty")
            }
            Self::PersonParticularsCorrectionReasonCannotBeEmpty => {
                write!(f, "a PersonParticulars correction reason must not be empty")
            }
            Self::PersonNameCorrectionReasonCannotBeEmpty => {
                write!(f, "a Person name correction reason must not be empty")
            }
            Self::PersonMasterDataDivergenceNotAcknowledged {
                person_id,
                diverging_periods,
            } => {
                write!(
                    f,
                    "correcting Person {person_id} needs the acknowledgement of every Live \
                     finalized PayPeriod it now diverges from: "
                )?;
                if diverging_periods.is_empty() {
                    return write!(f, "none");
                }
                for (index, period) in diverging_periods.iter().enumerate() {
                    if index > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{} to {}", period.start(), period.end())?;
                }
                Ok(())
            }
            Self::OperatorEmailCannotBeEmpty => {
                write!(f, "an Operator email must not be empty")
            }
            Self::OperatorDisplayNameCannotBeEmpty => {
                write!(f, "an Operator display name must not be empty")
            }
            Self::OperatorPasswordTooShort { minimum } => write!(
                f,
                "an Operator password must contain at least {minimum} characters"
            ),
            Self::OperatorPasswordTooLong { maximum } => write!(
                f,
                "an Operator password must contain at most {maximum} characters"
            ),
            Self::OperatorEmailAlreadyInUse => write!(
                f,
                "an Operator with this email already exists (email is unique case-insensitively)"
            ),
            Self::PasswordHashingFailed(message) => {
                write!(f, "password hashing failed: {message}")
            }
            Self::OperatorNotFound(id) => write!(f, "no Operator exists with id {id}"),
            Self::OperatorAlreadyDisabled(id) => write!(f, "Operator {id} is already disabled"),
            Self::OperatorCredentialInvalid => write!(f, "the Operator credential is invalid"),
            Self::EmployerMembershipAlreadyExists {
                operator_id,
                employer_id,
            } => write!(
                f,
                "Operator {operator_id} already has a membership for Employer {employer_id}"
            ),
            Self::EmployerMembershipNotFound {
                operator_id,
                employer_id,
            } => write!(
                f,
                "no EmployerMembership exists for Operator {operator_id} and Employer {employer_id}"
            ),
            Self::EmployerMembershipAlreadyRevoked {
                operator_id,
                employer_id,
            } => write!(
                f,
                "the membership for Operator {operator_id} and Employer {employer_id} is already revoked"
            ),
            Self::BootstrapOperatorAlreadyExists => write!(
                f,
                "bootstrap refused: an Operator already exists; bootstrap only ever creates the first one"
            ),
            Self::BootstrapOperatorTableBusy => write!(
                f,
                "bootstrap refused: the operator table is in use by something else; bootstrap is a \
                 first-run command and expects a database no server is running against"
            ),
            Self::BootstrapPeriodEndDayInvalid { day } => write!(
                f,
                "--period-end-day {day} is not a day of month in 1..=28; use a day in that range \
                 or \"last-day-of-month\""
            ),
            Self::StandingPayItemNotFound(id) => {
                write!(f, "no StandingPayItem exists with id {id}")
            }
            Self::StandingPayItemAlreadyEnded(id) => {
                write!(f, "StandingPayItem {id} is already ended")
            }
            Self::StandingPayItemEndReasonCannotBeEmpty => write!(
                f,
                "ending a StandingPayItem requires a reason that is not empty or only whitespace"
            ),
            Self::StandingMedicalAidPremiumIsZero => write!(
                f,
                "a standing medical aid premium of zero withholds nothing and is not a deduction"
            ),
        }
    }
}

impl std::error::Error for PayrollAppError {
    /// The wrapped error's own source, never the wrapped error itself.
    ///
    /// `Display` for the `Payroll` variant already prints the
    /// [`PayrollError`]'s message verbatim, so returning that same error as
    /// the source would make every chain-printing reporter — `tracing`,
    /// `anyhow`, a log line walking `source()` — print the one refusal
    /// twice. This is what `thiserror`'s `#[error(transparent)]` does,
    /// written out.
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Payroll(err) => err.source(),
            // Same reason as `Payroll` above: `Display` already prints the
            // wrapped refusal's message, so returning it as the source would
            // make every chain-printing reporter say it twice. Whatever sat
            // *below* it is still reachable.
            Self::FinalizationRebuildRefused { refusal, .. } => refusal.source(),
            Self::Database(_)
            | Self::SchemaOutOfDate { .. }
            | Self::EmployerNameCannotBeEmpty
            | Self::EmployerNotFound(_)
            | Self::EmploymentNotFound(_)
            | Self::PersonNotFound(_)
            | Self::PersonFullNameCannotBeEmpty
            | Self::EmploymentIsVoid(_)
            | Self::EmploymentEndsBeforeItStarts { .. }
            | Self::NoCompensationTermsInForce(_)
            | Self::PriorEmploymentDeclarationCannotBeUnknown
            | Self::UnsupportedDeductionDeclarationCannotBeUnknown
            | Self::SaltCoverageStartNotAPeriodEnd { .. }
            | Self::SaltCoverageStartOutsideTaxYear { .. }
            | Self::SaltCoverageStartBeforeEmploymentIsPayable { .. }
            | Self::OpeningBalanceFiguresOverAnEmptyCoveredSpan { .. }
            | Self::PayPeriodNotGeneratedByThePaySchedule { .. }
            | Self::PayrollRunNotFound(_)
            | Self::PayrollRunIsNotOrdinary(_)
            | Self::RemovalReasonCannotBeEmpty
            | Self::EmploymentNotAnActiveRunMember { .. }
            | Self::VoluntaryDeductionAmountIsZero { .. }
            | Self::PayrollRunAlreadyFinalized { .. }
            | Self::PayrollRunNotCalculated(_)
            | Self::FinalizationInputMismatch { .. }
            | Self::FinalizationRulesMismatch { .. }
            | Self::FinalizationCalculationMismatch { .. }
            | Self::FinalizedPayrollNotFound(_)
            | Self::FinalizedPayrollSnapshotUnreadable { .. }
            | Self::FinalizedPayrollAlreadyReversed(_)
            | Self::ReversalReasonCannotBeEmpty
            | Self::OpeningBalanceFrozenByFinalization { .. }
            | Self::PriorEmploymentFrozenByFinalization { .. }
            | Self::PayScheduleFrozenByFinalization { .. }
            | Self::PayScheduleChangeBlockedByAnOpenRun { .. }
            | Self::PayScheduleMovedWithinTaxYear { .. }
            | Self::PayScheduleChangeWouldStrandAStoredBoundary { .. }
            | Self::TaxYearOutsideRepresentableCalendar { .. }
            | Self::PrecedingPeriodUnresolved { .. }
            | Self::CorrectionReasonCannotBeEmpty
            | Self::PayrollRunIsNotCorrection(_)
            | Self::CorrectionRunAlreadyHasAnEmployment(_)
            | Self::CorrectionEmploymentBelongsToADifferentEmployer { .. }
            | Self::CorrectionTargetDoesNotMatch { .. }
            | Self::CorrectionTargetNotReversed(_)
            | Self::CorrectionTargetAlreadyReplaced(_)
            | Self::CorrectionLineageNotLegitimate { .. }
            | Self::CorrectionLineageOmitsAReversedPredecessor { .. }
            | Self::CorrectionPeriodAlreadyHasALivePayroll { .. }
            | Self::CompensationTermsCorrectionReasonCannotBeEmpty
            | Self::NoCompensationTermsRowAt { .. }
            | Self::UnsupportedDeductionDeclarationReasonCannotBeEmpty
            | Self::MasterDataDivergenceNotAcknowledged { .. }
            | Self::CompensationTermsAlreadyExistAt { .. }
            | Self::EmployerParticularsRegisteredNameCannotBeEmpty
            | Self::EmployerParticularsAddressLine1CannotBeEmpty
            | Self::EmployerParticularsCityCannotBeEmpty
            | Self::EmployerParticularsCorrectionReasonCannotBeEmpty
            | Self::EmployerMasterDataDivergenceNotAcknowledged { .. }
            | Self::PersonParticularsIdentityNumberCannotBeEmpty
            | Self::PersonParticularsAddressLine1CannotBeEmpty
            | Self::PersonParticularsCityCannotBeEmpty
            | Self::PersonParticularsCorrectionReasonCannotBeEmpty
            | Self::PersonNameCorrectionReasonCannotBeEmpty
            | Self::PersonMasterDataDivergenceNotAcknowledged { .. }
            | Self::OperatorEmailCannotBeEmpty
            | Self::OperatorDisplayNameCannotBeEmpty
            | Self::OperatorPasswordTooShort { .. }
            | Self::OperatorPasswordTooLong { .. }
            | Self::OperatorEmailAlreadyInUse
            | Self::PasswordHashingFailed(_)
            | Self::OperatorNotFound(_)
            | Self::OperatorAlreadyDisabled(_)
            | Self::OperatorCredentialInvalid
            | Self::EmployerMembershipAlreadyExists { .. }
            | Self::EmployerMembershipNotFound { .. }
            | Self::EmployerMembershipAlreadyRevoked { .. }
            | Self::BootstrapOperatorAlreadyExists
            | Self::BootstrapOperatorTableBusy
            | Self::BootstrapPeriodEndDayInvalid { .. }
            | Self::StandingPayItemNotFound(_)
            | Self::StandingPayItemAlreadyEnded(_)
            | Self::StandingPayItemEndReasonCannotBeEmpty
            | Self::StandingMedicalAidPremiumIsZero => None,
        }
    }
}

/// At most this many differing fields are named. A refusal is read by a
/// person deciding what to fix, and a list longer than this says "recalculate
/// and look again" more usefully than an exhaustive dump does.
const MAX_NAMED_DIFFERENCES: usize = 5;

/// Longer values are cut to this many characters. A `PayrollRules` holds
/// whole PAYE band tables, and an unbounded refusal message is one nobody
/// reads.
const MAX_VALUE_CHARS: usize = 60;

/// Names the fields that differ between what finalization approved and what
/// it rebuilt, for the three refusals §5.2 raises. ADR-0010 makes this the
/// requirement it is: the refusal must name **which of the three** differed
/// *and what changed inside it*, because "a refusal that says only
/// 'something changed' is one users learn to click past".
///
/// The two values are walked as JSON rather than compared field by field in
/// Rust. That is not a shortcut: JSON is exactly what a `FinalizedPayroll`
/// freezes and what `working_payroll_calculation` stores, so every path named
/// here is a path a reader of the stored snapshot can find, and a new field on
/// any of the three types is described without this function being touched.
///
/// `None` means the value could not be serialized. That cannot happen for the
/// three types this is called with — the same serialization is an `expect`
/// wherever they are written — but `Display` must not panic, so it is reported
/// rather than unwrapped.
fn describe_differences(
    approved: Option<serde_json::Value>,
    current: Option<serde_json::Value>,
) -> String {
    let (Some(approved), Some(current)) = (approved, current) else {
        return "the differing fields could not be described".to_string();
    };

    let mut differences = Vec::new();
    collect_differences("", &approved, &current, &mut differences);

    match differences.len() {
        0 => "the differing fields could not be described".to_string(),
        n if n > MAX_NAMED_DIFFERENCES => format!(
            "{}, and further fields differ",
            differences[..MAX_NAMED_DIFFERENCES].join("; ")
        ),
        _ => differences.join("; "),
    }
}

/// Walks two JSON values in step, pushing `path: approved X, current Y` for
/// every leaf that disagrees. Recurses into objects and into arrays of equal
/// length; anything else that differs is reported at the level it differs on,
/// so a changed Earning line reads as one difference rather than as every
/// field of every line after it.
///
/// Stops one past [`MAX_NAMED_DIFFERENCES`] — enough for the caller to know
/// the list was cut, without walking a whole PAYE table to say so.
fn collect_differences(
    path: &str,
    approved: &serde_json::Value,
    current: &serde_json::Value,
    out: &mut Vec<String>,
) {
    if approved == current || out.len() > MAX_NAMED_DIFFERENCES {
        return;
    }

    match (approved, current) {
        (serde_json::Value::Object(approved), serde_json::Value::Object(current)) => {
            let null = serde_json::Value::Null;
            let keys = approved
                .keys()
                .chain(current.keys().filter(|key| !approved.contains_key(*key)));
            for key in keys {
                let child = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                collect_differences(
                    &child,
                    approved.get(key).unwrap_or(&null),
                    current.get(key).unwrap_or(&null),
                    out,
                );
            }
        }
        (serde_json::Value::Array(approved), serde_json::Value::Array(current))
            if approved.len() == current.len() =>
        {
            for (index, (approved, current)) in approved.iter().zip(current).enumerate() {
                collect_differences(&format!("{path}[{index}]"), approved, current, out);
            }
        }
        _ => out.push(format!(
            "{}: approved {}, current {}",
            if path.is_empty() { "the value" } else { path },
            abbreviate(approved),
            abbreviate(current)
        )),
    }
}

/// One JSON value as compact text, cut to [`MAX_VALUE_CHARS`] characters.
/// Cut by characters rather than bytes: the cut must not land inside one.
fn abbreviate(value: &serde_json::Value) -> String {
    let text = value.to_string();
    if text.chars().count() <= MAX_VALUE_CHARS {
        return text;
    }
    let kept: String = text.chars().take(MAX_VALUE_CHARS).collect();
    format!("{kept}…")
}

impl From<PayrollError> for PayrollAppError {
    fn from(err: PayrollError) -> Self {
        Self::Payroll(err)
    }
}

impl From<sqlx::Error> for PayrollAppError {
    /// Reduced to its message rather than kept as a live [`sqlx::Error`]:
    /// that type is neither `Clone` nor comparable, and every other variant
    /// here is both, so callers can assert on a refusal by value.
    fn from(err: sqlx::Error) -> Self {
        Self::Database(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    fn refusal() -> PayrollError {
        PayrollError::PriorEmploymentUnknown
    }

    #[test]
    fn wrapping_preserves_the_refusals_message() {
        let wrapped = PayrollAppError::from(refusal());

        assert_eq!(wrapped.to_string(), refusal().to_string());
    }

    #[test]
    fn the_wrapper_never_repeats_the_refusal_in_its_source_chain() {
        let wrapped = PayrollAppError::from(refusal());

        let mut chain = Vec::new();
        let mut next: Option<&(dyn Error + 'static)> = wrapped.source();
        while let Some(err) = next {
            chain.push(err.to_string());
            next = err.source();
        }

        assert_eq!(chain, Vec::<String>::new());
    }

    #[test]
    fn a_difference_is_named_by_its_path_through_the_frozen_snapshot() {
        let approved = serde_json::json!({"employment": {"basic_pay": {"cents": 1500000}}});
        let current = serde_json::json!({"employment": {"basic_pay": {"cents": 1600000}}});

        assert_eq!(
            describe_differences(Some(approved), Some(current)),
            "employment.basic_pay.cents: approved 1500000, current 1600000"
        );
    }

    #[test]
    fn a_field_present_on_one_side_only_is_still_named() {
        let approved = serde_json::json!({"effective_from": "2026-03-01"});
        let current = serde_json::json!({});

        assert_eq!(
            describe_differences(Some(approved), Some(current)),
            "effective_from: approved \"2026-03-01\", current null"
        );
    }

    #[test]
    fn a_long_list_of_differences_is_cut_and_says_so() {
        let approved = serde_json::json!({
            "a": 1, "b": 1, "c": 1, "d": 1, "e": 1, "f": 1, "g": 1
        });
        let current = serde_json::json!({
            "a": 2, "b": 2, "c": 2, "d": 2, "e": 2, "f": 2, "g": 2
        });

        let described = describe_differences(Some(approved), Some(current));

        assert_eq!(described.matches("approved").count(), MAX_NAMED_DIFFERENCES);
        assert!(
            described.ends_with(", and further fields differ"),
            "{described}"
        );
    }

    #[test]
    fn a_long_value_is_abbreviated_rather_than_dumped() {
        let approved = serde_json::json!({"bands": "x".repeat(500)});
        let current = serde_json::json!({"bands": "y".repeat(500)});

        let described = describe_differences(Some(approved), Some(current));

        assert!(described.contains('…'), "{described}");
        assert!(described.chars().count() < 200, "{described}");
    }

    #[test]
    fn a_value_that_cannot_be_serialized_is_reported_rather_than_unwrapped() {
        assert_eq!(
            describe_differences(None, Some(serde_json::json!({}))),
            "the differing fields could not be described"
        );
    }

    #[test]
    fn the_question_mark_operator_converts_a_refusal() {
        fn use_case() -> Result<(), PayrollAppError> {
            Err(refusal())?;
            unreachable!()
        }

        assert_eq!(use_case(), Err(PayrollAppError::Payroll(refusal())));
    }
}
