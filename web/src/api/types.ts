// The request and response shapes salt-server's `/api/session` routes carry
// (crates/salt-server/src/session.rs). Hand-written per the spec's own
// instruction (§0.32 of docs/domain/operator-auth-http-web-grill.md): one
// file, no code generation, revisited only when the surface stops fitting
// in it.

export interface LoginRequest {
  email: string;
  password: string;
}

export type MembershipRole = 'owner' | 'payrollOperator';

export interface MembershipDto {
  employerId: string;
  name: string;
  role: MembershipRole;
}

export interface OperatorDto {
  id: string;
  email: string;
  displayName: string;
}

export interface SessionResponse {
  operator: OperatorDto;
  memberships: MembershipDto[];
}

// `crates/salt-server/src/employer_particulars.rs` (issue #71, parent #70
// D-7): the Employer's registered name, address and statutory registration
// numbers. `null` means this Employer has never recorded any.

export interface EmployerParticularsResponse {
  registeredName: string;
  addressLine1: string;
  addressLine2: string | null;
  city: string;
  postalCode: string | null;
  incomeTaxNumber: string | null;
  socialSecurityNumber: string | null;
  createdAt: string;
  createdBy: string;
}

/** `PUT .../particulars`. `acknowledgedDivergingPeriods` and `reason` default
 * to empty on the server, so a write that diverges from nothing can omit
 * both — see `DivergingPeriodsResponse` below for what a write returns. */
export interface SetEmployerParticularsRequest {
  registeredName: string;
  addressLine1: string;
  addressLine2?: string;
  city: string;
  postalCode?: string;
  incomeTaxNumber?: string;
  socialSecurityNumber?: string;
  acknowledgedDivergingPeriods?: PayPeriodDto[];
  reason?: string;
}

// `crates/salt-server/src/person_particulars.rs` (issue #72, parent #70
// D-7): a Person's identity number and address, plus every ActionLog entry a
// correction of either that or `fullName` has ever produced. Not
// Owner-only — any active member reads and writes this.

export interface ActionLogEntryDto {
  occurredAt: string;
  actor: string;
  actionType: string;
  context: Record<string, unknown> | null;
}

export interface PersonParticularsResponse {
  fullName: string;
  identityNumber: string | null;
  addressLine1: string | null;
  addressLine2: string | null;
  city: string | null;
  postalCode: string | null;
  particularsCreatedAt: string | null;
  particularsCreatedBy: string | null;
  actionLog: ActionLogEntryDto[];
}

/** `PUT .../particulars`. */
export interface SetPersonParticularsRequest {
  identityNumber: string;
  addressLine1: string;
  addressLine2?: string;
  city: string;
  postalCode?: string;
  acknowledgedDivergingPeriods?: PayPeriodDto[];
  reason?: string;
}

/** `PUT .../name`. Unlike `SetPersonParticularsRequest`, `reason` is never
 * optional here — the server refuses a blank one unconditionally. */
export interface CorrectPersonFullNameRequest {
  fullName: string;
  acknowledgedDivergingPeriods?: PayPeriodDto[];
  reason: string;
}

// `crates/salt-server/src/employments.rs` (issue #62, §0.38): the interface
// only ever sends `fullName`, never `personId` — there is no Person screen to
// have picked one from.

export interface EmploymentListingDto {
  employmentId: string;
  personId: string;
  fullName: string;
}

export interface EmploymentsResponse {
  employments: EmploymentListingDto[];
}

export interface CreateEmploymentRequest {
  fullName: string;
  startDate: string;
}

export interface CreateEmploymentResponse {
  employmentId: string;
  personId: string;
}

// `crates/salt-server/src/employments.rs`'s `get_employment` and
// `crates/salt-server/src/employment_facts.rs` (issue #63, §0.35): everything
// one Employment needs to become payable.

export interface EmploymentDetailResponse {
  employmentId: string;
  personId: string;
  fullName: string;
  startDate: string;
  endDate: string | null;
  currentBasicPayCents: number | null;
  currentOrdinaryHours: string | null;
}

export interface PayPeriodDto {
  start: string;
  end: string;
}

export interface DivergingPeriodsResponse {
  divergingPeriods: PayPeriodDto[];
}

/** `POST .../compensation-terms`. `acknowledgedDivergingPeriods` and `reason`
 * default to empty on the server, so a call that diverges from nothing can
 * omit both. */
export interface RecordCompensationTermsRequest {
  effectiveFrom: string;
  basicPayCents: number;
  ordinaryHours: string;
  acknowledgedDivergingPeriods?: PayPeriodDto[];
  reason?: string;
}

/** `"confirmed_none"` or `"present"` — the same two-valued status the server
 * stores. There is no wire spelling for "unknown": that is what a
 * declaration that was never made already means. */
export type PriorEmploymentStatus = 'confirmed_none' | 'present';

/** `POST .../prior-employment`. */
export interface DeclarePriorEmploymentRequest {
  taxYear: number;
  status: PriorEmploymentStatus;
  taxableRemunerationCents?: number;
  payeCents?: number;
}

export type UnsupportedDeductionStatusValue = 'confirmed_none' | 'present';

/** The four deduction kinds Salt does not calculate
 * (`crates/payroll/src/unsupported_deduction.rs`), by their stable wire code
 * (`crates/salt-server/src/payroll_error.rs`). */
export type UnsupportedDeductionKindCode =
  'approved_pension_fund' | 'provident_fund' | 'retirement_annuity_fund' | 'education_policy';

/** `POST .../unsupported-deductions`. Unlike compensation terms, `reason` is
 * never optional here — the server refuses a blank one unconditionally. */
export interface DeclareUnsupportedDeductionStatusRequest {
  effectiveFrom: string;
  status: UnsupportedDeductionStatusValue;
  kinds?: UnsupportedDeductionKindCode[];
  acknowledgedDivergingPeriods?: PayPeriodDto[];
  reason: string;
}

/** `POST .../opening-balance`. */
export interface RecordOpeningBalanceRequest {
  taxYear: number;
  saltCoverageStart: string;
  priorTaxableRemunerationCents: number;
  priorPayeCents: number;
}

/** The body of a route that records a fact and has nothing to report back. */
export type RecordedResponse = Record<string, never>;

// `crates/salt-server/src/payroll_runs.rs` (issue #64, parent #59 Spec 3 of
// 3, §0.22/§0.29/§0.31): an Employer's payroll runs, the members each one
// proposes to pay, and why a member cannot be paid yet.

export type PayrollRunStatus = 'draft' | 'calculated' | 'finalized';

/** `POST .../payroll-runs`. Ordinary runs only — there is no Correction-run
 * creation route (§0.22). */
export interface CreatePayrollRunRequest {
  period: PayPeriodDto;
  payDate: string;
}

export interface CreatePayrollRunResponse {
  payrollRunId: string;
}

export interface PayrollRunSummaryDto {
  payrollRunId: string;
  period: PayPeriodDto;
  payDate: string;
  status: PayrollRunStatus;
}

export interface PayrollRunsResponse {
  payrollRuns: PayrollRunSummaryDto[];
}

export type EarningKind = 'taxableAllowance' | 'overtime';
export type PayLineSourceDto = 'one_off' | 'from_reversed_snapshot';

/** A taxable allowance: money an Operator decided. */
export interface TaxableAllowanceLineDto {
  kind: 'taxableAllowance';
  amountCents: number;
  /** Null only for unlabelled lines preserved from a version-1 snapshot. */
  label: string | null;
  source: PayLineSourceDto;
}

/**
 * Overtime: hours at a multiplier, with **no amount** — Salt prices it from
 * the Employment's own pay and ordinary hours (ADR-0022). `hours` and
 * `multiplier` are decimal strings, never numbers, so no figure passes
 * through a JSON float.
 *
 * The multiplier set is closed at `'1.5'` and `'2'`. Whether those are the
 * correct and only statutory factors in Namibia is `Q-OPEN-8` and is not
 * verified — nothing on screen may present them as law.
 */
export interface OvertimeLineDto {
  kind: 'overtime';
  hours: string;
  multiplier: string;
  label: string | null;
  source: PayLineSourceDto;
}

export type EarningLineDto = TaxableAllowanceLineDto | OvertimeLineDto;

/**
 * The six blocker codes (§0.31) and no others — the run detail's own
 * contract. A blocker carries no `message` at all, only `code` and
 * `details`: the sentence an Operator reads is a lookup keyed by `code`,
 * never a rendering of a server string that may be reworded (§0.23, issue
 * #64's own Deep Instructions).
 */
export type PayrollRunBlockerCode =
  | 'prior_employment_unknown'
  | 'prior_employment_treatment_unconfirmed'
  | 'unsupported_deduction_status_unknown'
  | 'unsupported_deductions_present'
  | 'no_compensation_terms_in_force'
  | 'ordinary_hours_not_recorded';

export interface PayrollRunBlockerDto {
  code: PayrollRunBlockerCode;
  details: unknown;
}

/** The ten figures §0.29 names for a member's current calculation, cents-exact
 * (INV-001). `null` until the run has been calculated at least once.
 * `overtimeCents` is its own figure and never folded into
 * `taxableAllowancesCents`: overtime feeds PAYE and gross but never the
 * social security base. */
export interface FiguresDto {
  basicPayCents: number;
  taxableAllowancesCents: number;
  overtimeCents: number;
  grossCents: number;
  taxableRemunerationCents: number;
  payeCents: number;
  employeeSscCents: number;
  employerSscCents: number;
  totalDeductionsCents: number;
  netCents: number;
}

/** What Calculate's own most recent call said about one member. Never
 * persisted, so a plain `GET` always carries `null` here, even for a member
 * still blocked (§0.31). */
export interface RefusalDto {
  code: string;
  details: unknown;
}

export interface PayrollRunMemberDto {
  employmentId: string;
  finalizedPayrollId: string | null;
  fullName: string;
  earnings: EarningLineDto[];
  blockers: PayrollRunBlockerDto[];
  figures: FiguresDto | null;
  figuresAbsence: 'not_calculated' | 'pay_lines_changed' | null;
  refusal: RefusalDto | null;
}

export interface PayrollRunDetailResponse {
  payrollRunId: string;
  period: PayPeriodDto;
  payDate: string;
  status: PayrollRunStatus;
  members: PayrollRunMemberDto[];
}

// `crates/salt-server/src/payroll_runs.rs`'s `finalize_payroll_run` (issue
// #66, §0.22/§0.26/§0.28): the one atomic act that turns a Calculated run
// into immutable history. One entry per member the run just finalized.

export interface FinalizedMemberDto {
  employmentId: string;
  finalizedPayrollId: string;
}

export interface FinalizePayrollRunResponse {
  finalized: FinalizedMemberDto[];
}

// `GET /api/employers/{e}/finalized-payroll/{f}`
// (`crates/salt-server/src/finalized_payroll.rs`, issue #57/#66, §0.29): one
// immutable finalized payroll — the same ten figures a working run's own
// detail carries, plus the period, the pay date and the SaltVersion that
// produced them. Never the raw frozen snapshot (§0.29).
//
// `employerParticulars`, `personParticulars` and `payslipTemplateVersion`
// are issue #73's own three frozen fields: `null` on any of them means
// either nothing was on record to freeze at finalize time, or this payroll
// finalized before issue #73 shipped at all — the two read back
// indistinguishably, on purpose.

export interface FrozenEmployerParticularsDto {
  registeredName: string;
  addressLine1: string;
  addressLine2: string | null;
  city: string;
  postalCode: string | null;
  incomeTaxNumber: string | null;
  socialSecurityNumber: string | null;
}

export interface FrozenPersonParticularsDto {
  fullName: string;
  identityNumber: string | null;
  addressLine1: string | null;
  addressLine2: string | null;
  city: string | null;
  postalCode: string | null;
}

export interface FinalizedPayrollDetailResponse {
  finalizedPayrollId: string;
  employmentId: string;
  fullName: string;
  period: PayPeriodDto;
  payDate: string;
  figures: FiguresDto;
  saltVersion: string;
  employerParticulars: FrozenEmployerParticularsDto | null;
  personParticulars: FrozenPersonParticularsDto | null;
  payslipTemplateVersion: string | null;
}

// `GET /api/employers/{e}/finalized-payroll/{f}/traces`
// (`crates/salt-server/src/finalized_payroll.rs`, issue #67, §0.29): the
// PAYE and social security workings behind one finalized payroll's figures.
// `threshold`, `rate`, `tax` and `yearToDateTaxOwed` are decimal strings,
// never JSON floats, and are not cents — render them exactly as sent.

export interface BandContributionDto {
  threshold: string;
  rate: string;
  tax: string;
}

export interface PayeTraceDto {
  priorTaxableRemunerationCents: number;
  priorPayeCents: number;
  thisPeriodTaxableRemunerationCents: number;
  yearToDateTaxableRemunerationCents: number;
  yearToDateTaxOwed: string;
  bandsApplied: BandContributionDto[];
  periodsElapsed: number;
}

export type SscClampDto = 'none' | 'floor' | 'ceiling';

export interface SscTraceDto {
  basicPayCents: number;
  baseCents: number;
  clamp: SscClampDto;
  rate: string;
  floorCents: number;
  ceilingCents: number;
}

/**
 * One overtime line's workings. Every figure needed to redo
 * `basicPay x 12 / 52 / ordinaryHours x hours x multiplier` by hand.
 *
 * The derived rate is an exact numerator/denominator pair, not cents. A
 * fraction preserves repeating rates without rounding; the single rounding
 * on the line produced `amountCents`.
 *
 * `policyReference` and `policyStatus` are wire *codes*, like `clamp` — the
 * words an Operator reads are this app's, never the server's. What they must
 * never be rendered as is law.
 */
export interface OvertimeTraceDto {
  amountCents: number;
  label: string | null;
  basicPayCents: number;
  ordinaryHours: string;
  monthsPerYear: string;
  weeksPerYear: string;
  derivedHourlyRateNumerator: string;
  derivedHourlyRateDenominator: string;
  hours: string;
  multiplier: string;
  policyReference: string;
  policyStatus: string;
}

export interface FinalizedPayrollTracesResponse {
  paye: PayeTraceDto;
  employeeSsc: SscTraceDto;
  employerSsc: SscTraceDto;
  /** One entry per overtime line; empty for a salary-only payroll. */
  overtime: OvertimeTraceDto[];
}
