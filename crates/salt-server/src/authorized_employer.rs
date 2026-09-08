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

use axum::extract::{FromRequestParts, RawPathParams};
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
/// `crate::employments`'s three routes (issue #51) are the first
/// production routes to use this — every later Employer-scoped route
/// reuses it the same way.
#[derive(Debug)]
pub(crate) struct AuthorizedEmployerContext {
    operator_id: OperatorId,
    employer_id: AuthorizedEmployerId,
    #[allow(
        dead_code,
        reason = "read by require_role, unused until the first Owner-only route lands"
    )]
    role: MembershipRole,
}

impl AuthorizedEmployerContext {
    #[allow(
        dead_code,
        reason = "no route has needed the raw OperatorId over actor() yet"
    )]
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
            MembershipRole::Owner => self.role() == MembershipRole::Owner,
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

    /// Every Employer-scoped URL carries the named `employer_id` path
    /// segment (§0.22). Read it from all raw parameters rather than extracting
    /// `Path<String>`: Axum accepts that scalar only for a route with exactly
    /// one parameter, while the nested routes in Specs 2 and 3 also carry an
    /// Employment, PayrollRun, or FinalizedPayroll id. This keeps the same
    /// extractor valid for every protected route in the parent spec.
    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let path_params = RawPathParams::from_request_parts(parts, state)
            .await
            .map_err(|rejection| ApiError::internal(rejection.to_string()))?;
        let raw_employer_id = path_params
            .iter()
            .find_map(|(name, value)| (name == "employer_id").then_some(value))
            .ok_or_else(|| {
                ApiError::internal(
                    "a route using AuthorizedEmployerContext is missing its employer_id path segment",
                )
            })?;
        let token = session_token(&parts.headers).ok_or_else(ApiError::unauthenticated)?;

        match payroll_app::resolve_employer_access(state.db(), token, raw_employer_id, Utc::now())
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::router::{build_authorization_probe_router, build_router};
    use crate::state::AppState;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use payroll_app::{DatabaseConfig, MembershipRole, OperatorId, SaltDatabase};
    use serde_json::Value;
    use tower::ServiceExt;

    fn test_database_config() -> DatabaseConfig {
        let url = std::env::var("DATABASE_URL").expect(
            "DATABASE_URL must name an already-migrated database for salt-server's own tests \
             (see crates/salt-server/src/authorized_employer.rs)",
        );
        DatabaseConfig {
            url,
            max_connections: 2,
            acquire_timeout: Duration::from_secs(10),
            idle_timeout: None,
        }
    }

    async fn test_db() -> SaltDatabase {
        SaltDatabase::connect(&test_database_config())
            .await
            .expect("connect to the local, migrated test database (see AGENTS.md)")
    }

    async fn router() -> Router {
        build_authorization_probe_router(AppState::new(test_db().await, true))
    }

    fn unique_email(local_part: &str) -> String {
        format!("{local_part}-{}@example.com", uuid::Uuid::new_v4())
    }

    async fn create_operator(email: &str) -> OperatorId {
        payroll_app::create_operator(
            &test_db().await,
            email,
            "Test Operator",
            "correct horse battery staple",
        )
        .await
        .unwrap()
    }

    async fn create_employer() -> String {
        payroll_app::create_employer(
            &test_db().await,
            "Acme Corp",
            payroll::PaySchedule::new(payroll::PeriodEndDay::LastDayOfMonth),
            "test-setup",
        )
        .await
        .unwrap()
        .to_string()
    }

    async fn login(email: &str) -> String {
        let response = router()
            .await
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/session")
                    .header("x-salt-request", "1")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "email": email,
                            "password": "correct horse battery staple",
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let set_cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .expect("a successful login always sets the session cookie")
            .to_str()
            .unwrap();
        set_cookie
            .split(';')
            .next()
            .expect("Set-Cookie always carries at least the name=value pair")
            .to_string()
    }

    fn probe_request(employer_id: &str, cookie: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder()
            .method("GET")
            .uri(format!("/__test/authorized-employer/{employer_id}"));
        if let Some(cookie) = cookie {
            builder = builder.header(header::COOKIE, cookie);
        }
        builder.body(Body::empty()).unwrap()
    }

    fn owner_only_probe_request(employer_id: &str, cookie: &str) -> Request<Body> {
        Request::builder()
            .method("GET")
            .uri(format!(
                "/__test/authorized-employer/{employer_id}/owner-only"
            ))
            .header(header::COOKIE, cookie)
            .body(Body::empty())
            .unwrap()
    }

    fn nested_probe_request(employer_id: &str, cookie: &str) -> Request<Body> {
        Request::builder()
            .method("GET")
            .uri(format!(
                "/__test/authorized-employer/{employer_id}/resources/known-resource"
            ))
            .header(header::COOKIE, cookie)
            .body(Body::empty())
            .unwrap()
    }

    async fn body_json(response: axum::response::Response) -> Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn an_unauthenticated_request_answers_401() {
        let employer_id = create_employer().await;

        let response = router()
            .await
            .oneshot(probe_request(&employer_id, None))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let json = body_json(response).await;
        assert_eq!(json["error"]["code"], "unauthenticated");
    }

    #[tokio::test]
    async fn the_production_router_never_mounts_the_authorization_probe() {
        let employer_id = create_employer().await;

        let response = build_router(AppState::new(test_db().await, true))
            .oneshot(probe_request(&employer_id, None))
            .await
            .unwrap();

        // If the extractor probe were mounted this request would be 401.
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_forged_session_cookie_answers_401() {
        let employer_id = create_employer().await;

        let response = router()
            .await
            .oneshot(probe_request(
                &employer_id,
                Some("salt_session=not-a-real-token"),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_member_reaches_their_own_employer_and_the_context_carries_the_right_facts() {
        let db = test_db().await;
        let email = unique_email("alice");
        let operator_id = create_operator(&email).await;
        let employer_id = create_employer().await;
        payroll_app::create_employer_membership(
            &db,
            &operator_id,
            &payroll::EmployerId::new(employer_id.clone()),
            MembershipRole::PayrollOperator,
        )
        .await
        .unwrap();
        let cookie = login(&email).await;

        let response = router()
            .await
            .oneshot(probe_request(&employer_id, Some(&cookie)))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let json = body_json(response).await;
        assert_eq!(json["operatorId"], operator_id.as_str());
        assert_eq!(json["employerId"], employer_id);
        assert_eq!(json["role"], "payrollOperator");
        assert_eq!(json["actor"], format!("operator:{operator_id}"));
    }

    #[tokio::test]
    async fn the_extractor_reads_employer_id_from_a_nested_multi_parameter_route() {
        let db = test_db().await;
        let email = unique_email("alice");
        let operator_id = create_operator(&email).await;
        let employer_id = create_employer().await;
        payroll_app::create_employer_membership(
            &db,
            &operator_id,
            &payroll::EmployerId::new(employer_id.clone()),
            MembershipRole::Owner,
        )
        .await
        .unwrap();
        let cookie = login(&email).await;

        let response = router()
            .await
            .oneshot(nested_probe_request(&employer_id, &cookie))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let json = body_json(response).await;
        assert_eq!(json["employerId"], employer_id);
    }

    #[tokio::test]
    async fn a_non_member_gets_404_for_an_employer_that_does_exist() {
        let email = unique_email("alice");
        create_operator(&email).await;
        let employer_id = create_employer().await;
        let cookie = login(&email).await;

        let response = router()
            .await
            .oneshot(probe_request(&employer_id, Some(&cookie)))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let json = body_json(response).await;
        assert_eq!(json["error"]["code"], "not_found");
    }

    #[tokio::test]
    async fn an_unknown_employer_id_also_answers_404_not_401() {
        let email = unique_email("alice");
        create_operator(&email).await;
        let cookie = login(&email).await;

        let response = router()
            .await
            .oneshot(probe_request("no-such-employer", Some(&cookie)))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_payroll_operator_is_refused_403_on_a_route_that_demands_owner() {
        let db = test_db().await;
        let email = unique_email("alice");
        let operator_id = create_operator(&email).await;
        let employer_id = create_employer().await;
        payroll_app::create_employer_membership(
            &db,
            &operator_id,
            &payroll::EmployerId::new(employer_id.clone()),
            MembershipRole::PayrollOperator,
        )
        .await
        .unwrap();
        let cookie = login(&email).await;

        let response = router()
            .await
            .oneshot(owner_only_probe_request(&employer_id, &cookie))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let json = body_json(response).await;
        assert_eq!(json["error"]["code"], "forbidden");
    }

    #[tokio::test]
    async fn an_owner_is_let_through_a_route_that_demands_owner() {
        let db = test_db().await;
        let email = unique_email("alice");
        let operator_id = create_operator(&email).await;
        let employer_id = create_employer().await;
        payroll_app::create_employer_membership(
            &db,
            &operator_id,
            &payroll::EmployerId::new(employer_id.clone()),
            MembershipRole::Owner,
        )
        .await
        .unwrap();
        let cookie = login(&email).await;

        let response = router()
            .await
            .oneshot(owner_only_probe_request(&employer_id, &cookie))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn a_membership_revoked_mid_session_is_refused_on_the_next_request() {
        let db = test_db().await;
        let email = unique_email("alice");
        let operator_id = create_operator(&email).await;
        let employer_id = create_employer().await;
        let employer_id_typed = payroll::EmployerId::new(employer_id.clone());
        payroll_app::create_employer_membership(
            &db,
            &operator_id,
            &employer_id_typed,
            MembershipRole::Owner,
        )
        .await
        .unwrap();
        let cookie = login(&email).await;

        assert_eq!(
            router()
                .await
                .oneshot(probe_request(&employer_id, Some(&cookie)))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );

        payroll_app::revoke_employer_membership(&db, &operator_id, &employer_id_typed)
            .await
            .unwrap();

        let response = router()
            .await
            .oneshot(probe_request(&employer_id, Some(&cookie)))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn an_operator_disabled_mid_session_is_refused_401_on_the_next_request() {
        let db = test_db().await;
        let email = unique_email("alice");
        let operator_id = create_operator(&email).await;
        let employer_id = create_employer().await;
        payroll_app::create_employer_membership(
            &db,
            &operator_id,
            &payroll::EmployerId::new(employer_id.clone()),
            MembershipRole::Owner,
        )
        .await
        .unwrap();
        let cookie = login(&email).await;

        assert_eq!(
            router()
                .await
                .oneshot(probe_request(&employer_id, Some(&cookie)))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );

        payroll_app::disable_operator(&db, &operator_id)
            .await
            .unwrap();

        let response = router()
            .await
            .oneshot(probe_request(&employer_id, Some(&cookie)))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let json = body_json(response).await;
        assert_eq!(json["error"]["code"], "unauthenticated");
    }
}
