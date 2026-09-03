//! The stateful application layer for Salt, built on the pure `payroll`
//! crate (ADR-0009). `payroll-app` depends on `payroll`; the reverse
//! dependency does not exist.
//!
//! INV-001 (money is exact decimal, never `f32`/`f64`) reaches this crate
//! too: the ban is declared once in the workspace root's `Cargo.toml`
//! under `[workspace.lints]`, so a member cannot opt out of it by
//! forgetting an attribute.

mod action_log;
mod calculate;
mod compensation_terms;
mod correction;
mod database;
mod employer;
mod employer_access;
mod employment;
mod error;
mod finalize;
mod freeze;
mod ids;
mod membership;
mod opening_balance;
mod operator;
mod payroll_run;
mod prior_employment;
mod reversal;
mod sequencing;
mod session;
mod unsupported_deduction_status;
mod year_to_date;

pub use action_log::ActionType;
pub use calculate::{PayrollRunCalculationRefusal, calculate_payroll_run};
pub use compensation_terms::{correct_compensation_terms, record_compensation_terms};
pub use correction::{EarningPrePopulation, add_employment_to_correction_run};
pub use database::{DatabaseConfig, SaltDatabase, ping};
pub use employer::{
    EmployerSummary, change_pay_schedule, create_employer, list_employers_for_operator,
};
pub use employer_access::{EmployerAccess, resolve_employer_access};
pub use employment::{create_employment, get_employment_snapshot, void_employment};
pub use error::{PayrollAppError, ScheduleBoundedFact};
pub use finalize::{
    FinalizationOutcome, FinalizedPayrollId, SNAPSHOT_SCHEMA_VERSION, finalize_payroll_run,
};
pub use membership::{
    EmployerMembershipSnapshot, MembershipRole, MembershipStatus, active_membership_role,
    create_employer_membership, list_employer_memberships, revoke_employer_membership,
};
pub use opening_balance::record_opening_balance;
pub use operator::{
    OperatorId, OperatorSnapshot, OperatorStatus, create_operator, disable_operator,
    find_operator_by_email, find_operator_by_id, verify_operator_credential,
};
pub use payroll_run::{
    PayrollRunId, create_correction_run, create_ordinary_payroll_run, remove_employment_from_run,
    set_run_earnings,
};
pub use prior_employment::{declare_prior_employment, get_prior_employment};
pub use reversal::reverse_finalized_payroll;
pub use session::{
    CreatedSession, SessionId, SessionSnapshot, create_session, create_session_for_active_operator,
    delete_session, load_session,
};
pub use unsupported_deduction_status::{
    declare_unsupported_deduction_status, get_unsupported_deduction_status,
};
pub use year_to_date::build_year_to_date_context;

/// The one type this crate re-exports straight from `payroll` rather than
/// wrapping in a snapshot of its own: `salt-server`'s
/// `AuthorizedEmployerContext` extractor (issue #47) must *construct* an
/// `EmployerId` from a URL path segment before it can ask
/// [`resolve_employer_access`] about it, not merely receive one back from a
/// use case the way every other caller across this seam does — and
/// `salt-server` depends on no crate but this one (ADR-0018's own boundary).
pub use payroll::EmployerId;

/// The Salt release that produced this build: semver plus a short git SHA in
/// one string, e.g. `0.1.0+g1a2b3c4` (CONTEXT.md, `SaltVersion`). Lets a
/// future maintainer find the exact code behind a historical figure.
///
/// Captured once by `build.rs` at build time and baked in here; never read
/// from git at run time, because a deployed binary has no repository. A
/// release build from a dirty working tree fails rather than record a SHA
/// the binary does not match.
pub const SALT_VERSION: &str = env!("SALT_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn salt_version_is_the_crate_semver_plus_a_short_git_sha() {
        let (semver, sha_part) = SALT_VERSION
            .split_once('+')
            .expect("SaltVersion must carry a build-metadata segment introduced by '+'");

        assert_eq!(
            semver,
            env!("CARGO_PKG_VERSION"),
            "the quoted half of a SaltVersion must be this crate's own version"
        );
        let mut parts = semver.split('.');
        for field in ["major", "minor", "patch"] {
            let value = parts
                .next()
                .unwrap_or_else(|| panic!("expected a semver like 0.1.0, got {semver}"));
            assert!(
                !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()),
                "expected a numeric {field} in {semver}"
            );
        }
        assert!(
            parts.next().is_none(),
            "expected exactly three semver fields, got {semver}"
        );

        let sha = sha_part.strip_prefix('g').unwrap_or_else(|| {
            panic!("expected the git SHA segment to start with 'g', got {sha_part}")
        });
        // `git rev-parse --short=8` widens the abbreviation past eight
        // characters only when eight would be ambiguous, so this is a floor.
        assert!(
            sha.len() >= 8,
            "expected at least an 8-character short SHA, got {sha}"
        );
        assert!(
            sha.bytes().all(|b| b.is_ascii_hexdigit()),
            "expected the SHA segment to be hexadecimal, got {sha}"
        );
    }
}
