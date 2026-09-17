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

/** `PUT .../end-date` (issue #81): an operator records a Leaver's end date
 * with a stated reason. Returns the Employment's own detail, refreshed. */
export interface RecordEmploymentEndDateRequest {
  endDate: string;
  reason: string;
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
  | 'approved_pension_fund'
  | 'provident_fund'
  | 'retirement_annuity_fund'
  | 'education_policy'
  | 'employer_paid_medical_aid';

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

// `crates/salt-server/src/standing_pay_items.rs` (issue #79, parent #70
// §D-5): an Employment's effective-dated StandingPayItems. Every new
// Ordinary run proposes the ones in force at its period end.

/** What a StandingPayItem holds — the same spellings a run's pay lines use.
 * A standing allowance always carries a label; overtime is never standing. */
export type StandingPayItemInstructionDto =
  | { kind: 'taxableAllowance'; amountCents: number; label: string }
  | { kind: 'medicalAidPremium'; amountCents: number };

/** `POST .../standing-pay-items`. `effectiveFrom` must be a pay period start. */
export type CreateStandingPayItemRequest = StandingPayItemInstructionDto & {
  effectiveFrom: string;
};

export interface CreateStandingPayItemResponse {
  standingPayItemId: string;
}

/** `POST .../standing-pay-items/{s}/end`. The server refuses a blank reason. */
export interface EndStandingPayItemRequest {
  reason: string;
}

/** How an item was ended. Ending never deletes it: a past proposal still
 * points at it. */
export interface StandingPayItemEndingDto {
  endedAt: string;
  endedBy: string;
  reason: string;
}

export type StandingPayItemDto = StandingPayItemInstructionDto & {
  standingPayItemId: string;
  effectiveFrom: string;
  createdAt: string;
  createdBy: string;
  /** `null` while the item is in force. */
  ended: StandingPayItemEndingDto | null;
};

/** `GET .../standing-pay-items`: every item, ended ones included. */
export interface StandingPayItemsResponse {
  standingPayItems: StandingPayItemDto[];
}

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

/**
 * Where a stored pay line came from (issue #77/#79): typed directly on this
 * run, copied from a reversed payroll's frozen snapshot, or proposed from a
 * StandingPayItem. Recorded by the server and returned, never sent — a write
 * carries instructions only.
 */
export type PayLineSourceDto = 'one_off' | 'from_reversed_snapshot' | 'standing';

/** The immutable identity and start date a standing proposed line names.
 * Both fields arrive exactly when `source` is `standing`, making that
 * provenance a truthful wire invariant rather than optional display data.
 *
 * `overrideReason` (issue #80) is present only when this run's line has
 * been changed from the item's own instruction; `standingPayLine` — the
 * item's own current instruction, `Line` because it differs in shape
 * between an earning and a deduction line — is present on every standing
 * line, overridden or not, so the worksheet can always say "the standing
 * amount is X" beside a line that may or may not still match it. */
export interface StandingPayLineProvenanceDto<Line> {
  source: 'standing';
  standingPayItemId: string;
  standingEffectiveFrom: string;
  overrideReason?: string;
  standingPayLine: Line;
}

type NonStandingPayLineProvenanceDto = {
  source: Exclude<PayLineSourceDto, 'standing'>;
};

/** A taxable allowance: money an Operator decided. */
export interface TaxableAllowanceLineDto {
  kind: 'taxableAllowance';
  amountCents: number;
  /** Null only for unlabelled lines preserved from a version-1 snapshot. */
  label: string | null;
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
}

/** One earning instruction: the shape `PUT .../pay-lines` accepts. */
export type EarningLineDto = TaxableAllowanceLineDto | OvertimeLineDto;

/** One stored line as the run detail returns it: its instruction and the
 * source the server recorded beside it. Overtime cannot be standing (D14),
 * and a standing allowance's own instruction is another allowance, so the
 * type preserves those server invariants instead of admitting impossible
 * combinations every consumer then has to defend. */
export type PayLineDto =
  | (TaxableAllowanceLineDto &
      (StandingPayLineProvenanceDto<TaxableAllowanceLineDto> | NonStandingPayLineProvenanceDto))
  | (OvertimeLineDto & NonStandingPayLineProvenanceDto);

/**
 * A medical aid premium withheld from the employee's own pay (issue #78) —
 * a `VoluntaryDeduction`, computed after PAYE and social security and
 * carrying no relief against taxable income (`SC-OPEN-7`,
 * `NEEDS NAMRA CONFIRMATION`). Exactly one kind exists today, the same as
 * the server-side enum it mirrors: a second kind is a code change on both
 * sides, never a free-text deduction type.
 */
export interface MedicalAidPremiumLineDto {
  kind: 'medicalAidPremium';
  amountCents: number;
}

/** One voluntary deduction instruction: the shape `PUT .../pay-lines`
 * accepts in its `deductions` array. */
export type DeductionLineDto = MedicalAidPremiumLineDto;

/** One stored deduction line as the run detail returns it. */
export type DeductionPayLineDto = DeductionLineDto &
  (StandingPayLineProvenanceDto<DeductionLineDto> | NonStandingPayLineProvenanceDto);

/** One removed standing line (issue #80, §0): it contributes nothing to
 * calculation, but stays visible with its reason. Always standing-sourced
 * — the database admits no other kind of removal — so `standingPayItemId`
 * and `standingEffectiveFrom` are never absent here, unlike on
 * `PayLineDto`/`DeductionPayLineDto`. Kept out of `earnings`/`deductions`
 * entirely: the two arrays a caller reads are exactly the arrays it must
 * send back to `PUT .../pay-lines`, so a removed line can never be resent
 * as a new one-off line. */
export type RemovedPayLineDto = (EarningLineDto | DeductionLineDto) & {
  standingPayItemId: string;
  standingEffectiveFrom: string;
  removedReason: string;
  /** Present when this line was overridden before it was removed — both
   * facts survive together. */
  overrideReason?: string;
};

/** One `StandingPayItem` named in the change signal or the refresh report
 * (issue #80): its id, when it began, and its own current instruction —
 * never an override — flattened in the same shape `standing-pay-items`
 * already gives one. */
export type StandingItemProposalDto = StandingPayItemInstructionDto & {
  standingPayItemId: string;
  effectiveFrom: string;
};

/** How a member's standing items in force now differ from what this draft
 * proposes (issue #80, §D-6). Always present; every array may be empty,
 * which itself means "nothing changed". Always empty for a Correction run
 * or an already-`Finalized` run. */
export interface StandingItemsChangedDto {
  /** In force now, no line at all yet — a removed line still counts as
   * "having a line", so it is never reported as an addition. */
  added: StandingItemProposalDto[];
  /** A plain, active, non-overridden line whose own instruction no longer
   * matches its item's — unreachable through any write this build makes
   * today (a `StandingPayItem` is immutable except for ending it), but
   * still computed and shown if it were ever true. */
  changed: StandingItemProposalDto[];
  /** An active (not removed) line whose item is no longer in force —
   * ended, most likely. Reported, never silently dropped: the line stays
   * exactly where it is until the operator removes it. */
  ended: StandingItemProposalDto[];
}

/**
 * Whether a member's `figures` are current, and why not when absent (§D-6).
 * `pay_lines_saved` means a pay-line write retired figures that existed; the
 * worksheet says so rather than showing numbers older than the lines.
 */
export type CalculationStateDto = 'current' | 'not_calculated' | 'pay_lines_saved';

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

/** The ten figures §0.29 names for a member's current calculation, plus the
 * medical aid premium issue #78 adds, cents-exact (INV-001). `null` until
 * the run has been calculated at least once.
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
  /** Every medical aid premium withheld, summed (issue #78) — its own
   * classified figure, after the two statutory ones, so `totalDeductionsCents`
   * is never the only place a voluntary deduction shows. */
  medicalAidPremiumCents: number;
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
  earnings: PayLineDto[];
  deductions: DeductionPayLineDto[];
  /** Every removed standing line, earnings and deductions together (issue
   * #80, §0's decision 3). Never in `earnings`/`deductions`. */
  removedPayLines: RemovedPayLineDto[];
  blockers: PayrollRunBlockerDto[];
  figures: FiguresDto | null;
  calculationState: CalculationStateDto;
  refusal: RefusalDto | null;
  /** A joiner or leaver in this period: `BasicPay` is prorated by employed
   * days, and nothing else is (issue #79). */
  basicPayProrated: boolean;
  /** How this member's standing items in force now differ from what this
   * draft proposes (issue #80). */
  standingItemsChanged: StandingItemsChangedDto;
}

/** `POST .../pay-lines/{s}/override` (issue #80): changes a proposed
 * standing line for this run only, with a reason, leaving the
 * `StandingPayItem` itself untouched. `line` is validated the same way a
 * new standing item is — an `overtime` kind is a 400, since no
 * `StandingPayItem` can ever be one. */
export interface OverrideStandingPayLineRequest {
  line: StandingPayItemInstructionDto;
  reason: string;
}

/** `POST .../pay-lines/{s}/remove` (issue #80). The server refuses a blank
 * reason. */
export interface RemoveStandingPayLineRequest {
  reason: string;
}

/** One member's own report from a refresh: what was added and updated, and
 * what was deliberately left alone. */
export interface MemberProposalRefreshDto {
  employmentId: string;
  added: StandingItemProposalDto[];
  updated: StandingItemProposalDto[];
  keptOverridden: string[];
  keptRemoved: string[];
  endedStillProposed: StandingItemProposalDto[];
}

/** `POST .../refresh-proposals` (issue #80): the one explicit,
 * operator-triggered act that catches a draft's proposals up with the
 * standing records — never automatic. Only members with something to
 * report are listed at all, so an empty `members` array itself means
 * "nothing changed; nothing was written". */
export interface RefreshStandingProposalsResponse {
  members: MemberProposalRefreshDto[];
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

/** One line's frozen provenance (issue #80): its own instruction, its
 * source, and — when it was a standing line — the item it named, that
 * item's own current instruction, and any override or removal reason
 * recorded for this run. Frozen at finalization and never rebuilt from
 * today's standing records (ADR-0004): an old payroll's workings can
 * always say "this was a one-month override, reason X", however the
 * standing record reads now. */
export type FrozenPayLineDto = (EarningLineDto | DeductionLineDto) & {
  source: PayLineSourceDto;
  standingPayItemId?: string;
  standingEffectiveFrom?: string;
  standingPayLine?: EarningLineDto | DeductionLineDto;
  overrideReason?: string;
  removedReason?: string;
};

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
  /** The frozen pay-line workings (issue #80) — `null` for a payroll
   * finalized before this shipped. */
  payLineProvenance: FrozenPayLineDto[] | null;
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

// `GET /api/employers/{e}/payroll-runs/{r}/register` and `GET
// .../payroll-runs/{r}/payment-summary` (`crates/salt-server/src/
// run_outputs.rs`, issue #83, parent #70 §D-9, §D-10): views over a
// finalized run's own `FinalizedPayroll` rows — every row this run
// produced (Register) or only its live ones (PaymentSummary). Neither
// route recomputes a figure; both read the frozen `FiguresDto` this app
// already renders everywhere else.

export type PayrollRunKindDto = 'ordinary' | 'correction';

/** One `FinalizedPayroll` row's liveness, tagged by `state` (README.md's
 * own rule: this JSON carries facts only, never the Operator-facing
 * sentence — that is built from `state`/`replacedBy`/`replaces` on the
 * client, the same way `LivenessDto`'s Rust twin documents it). */
export type LivenessDto =
  | { state: 'live' }
  | { state: 'reversed'; reason: string; reversedAt: string; replacedBy: string | null };

export interface PayrollRegisterRowDto {
  finalizedPayrollId: string;
  employmentId: string;
  fullName: string;
  figures: FiguresDto;
  liveness: LivenessDto;
  /** The finalized payroll this row replaces, when this row is itself a
   * replacement — independent of `liveness`, which is this row's own. */
  replaces: string | null;
}

export interface PayrollRegisterResponse {
  payrollRunId: string;
  kind: PayrollRunKindDto;
  period: PayPeriodDto;
  payDate: string;
  rows: PayrollRegisterRowDto[];
  /** Every row this run produced, a reversed one included — never
   * recomputed, the same eleven-figure shape `FiguresDto` already carries. */
  totalAsFinalized: FiguresDto;
  /** Only the rows still live from this run — a reversed row is never
   * counted here, so nothing is ever counted in both totals. */
  totalStillLive: FiguresDto;
}

export interface PaymentSummaryRowDto {
  finalizedPayrollId: string;
  employmentId: string;
  fullName: string;
  netPayCents: number;
  /** The finalized payroll this row replaces, when this row is itself a
   * replacement (§D-10: shown at full net pay, never a difference). */
  replaces: string | null;
}

export interface PaymentSummaryResponse {
  payrollRunId: string;
  kind: PayrollRunKindDto;
  period: PayPeriodDto;
  payDate: string;
  /** Live records only — a reversed `FinalizedPayroll` never appears here. */
  rows: PaymentSummaryRowDto[];
  excludedReversedCount: number;
  totalNetPayCents: number;
}
