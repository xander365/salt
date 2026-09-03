//! `AuthorizedEmployerContext` (issue #47, parent #38 Spec 1 of 3; ADR-0017;
//! `docs/domain/operator-auth-http-web-grill.md` §0.7-§0.10). An Axum
//! extractor a handler names in its arguments to get proof of who is
//! calling, which Employer they are authorized for, and with what role — and
//! the *only* way a handler ever obtains an Employer capability to act on.
//! A handler that skips this extractor cannot construct an
//! [`AuthorizedEmployerId`] from a path segment, so it cannot pass one to a
//! future Employer-scoped `payroll-app` use case.
//!
//! The one joined query behind it — session, Operator status and
//! `EmployerMembership`, with no caching — lives in `payroll-app`
//! ([`payroll_app::resolve_employer_access`]), the same ADR-0018 boundary
//! every other query in this crate already respects: this file reads no SQL
//! of its own, only the enum that query answers with.

use axum::extract::{FromRequestParts, Path};
use axum::http::request::Parts;
use chrono::Utc;
use payroll_app::{AuthorizedEmployerId, EmployerAccess, MembershipRole, OperatorId};

use crate::error::ApiError;
use crate::session::session_token;
use crate::state::AppState;

/// Proof that the caller is signed in, active, and holds an active
/// `EmployerMembership` for `employer_id` — resolved fresh on every request,
/// never cached (ADR-0017). A handler takes the authorized Employer
/// capability it needs out of this struct rather than out of the URL path
/// directly, and calls
/// [`Self::require_role`] when the route is one of the Owner-only ones §0.6
/// names; a route not on that list checks no role at all.
///
/// This spec ships zero Employer-scoped production routes (its own Deep
/// Instructions), so nothing in the binary build constructs one yet — issue
/// #47's own acceptance criterion is proving the extractor through
/// `router.rs`'s `test-support`-only probe routes instead. The first real
/// route in a later spec is what removes the need for this `allow`.
#[derive(Debug)]
#[allow(
    dead_code,
    reason = "unused outside test-support until the first Employer-scoped route lands"
)]
pub(crate) struct AuthorizedEmployerContext {
    operator_id: OperatorId,
    employer_id: AuthorizedEmployerId,
    role: MembershipRole,
}

#[allow(
    dead_code,
    reason = "unused outside test-support until the first Employer-scoped route lands"
)]
impl AuthorizedEmployerContext {
    pub(crate) fn operator_id(&self) -> &OperatorId {
        &self.operator_id
    }

    pub(crate) fn employer_id(&self) -> &AuthorizedEmployerId {
        &self.employer_id
    }

    pub(crate) fn role(&self) -> MembershipRole {
        self.role
    }

    /// `operator:<OperatorId>` (ADR-0019, §0.11) — the only actor
    /// `payroll-app`'s ActionLog is ever handed once a later spec starts
    /// writing entries from HTTP, taken from this context and never from a
    /// request body.
    pub(crate) fn actor(&self) -> String {
        format!("operator:{}", self.operator_id)
    }

    /// Refuses with 403 unless [`Self::role`] meets `required` — `Owner`
    /// satisfies only `Owner`, and `PayrollOperator` satisfies itself (§0.6:
    /// "Everything payroll is `PayrollOperator` or above"). Never 404: by
    /// the time a handler can call this, [`payroll_app::resolve_employer_access`]
    /// has already proven the caller *is* a member, so refusing here leaks
    /// nothing ADR-0017 protects.
    pub(crate) fn require_role(&self, required: MembershipRole) -> Result<(), ApiError> {
        let sufficient = match required {
            MembershipRole::PayrollOperator => true,
            MembershipRole::Owner => self.role == MembershipRole::Owner,
        };
        if sufficient {
            Ok(())
        } else {
            Err(ApiError::forbidden())
        }
    }
}

impl FromRequestParts<AppState> for AuthorizedEmployerContext {
    type Rejection = ApiError;

    /// Every Employer-scoped URL carries the EmployerId as a path segment
    /// (§0.22), so this reads it the same way any handler taking `Path<T>`
    /// would — the difference is that this is the *only* place in the crate
    /// allowed to turn that raw segment into something a handler can act on,
    /// because turning it into one is exactly the step that must go through
    /// [`payroll_app::resolve_employer_access`] first.
    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let Path(raw_employer_id) = Path::<String>::from_request_parts(parts, state)
            .await
            .map_err(|_rejection| {
                ApiError::internal(
                    "a route using AuthorizedEmployerContext is missing its EmployerId path segment",
                )
            })?;
        let token = session_token(&parts.headers).ok_or_else(ApiError::unauthenticated)?;

        match payroll_app::resolve_employer_access(state.db(), token, &raw_employer_id, Utc::now())
            .await
            .map_err(ApiError::internal)?
        {
            // Indistinguishable from outside (ADR-0017: a 403 here would
            // confirm the Employer exists), so this is the identical 404 an
            // unmatched route gets — not merely the same status code.
            EmployerAccess::Unauthenticated => Err(ApiError::unauthenticated()),
            EmployerAccess::NotAMember => Err(ApiError::not_found()),
            EmployerAccess::Member {
                operator_id,
                employer_id,
                role,
            } => Ok(Self {
                operator_id,
                employer_id,
                role,
            }),
        }
    }
}
