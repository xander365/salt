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
  | 'education_policy';

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
