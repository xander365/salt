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
