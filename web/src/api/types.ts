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
