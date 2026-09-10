//! `SaltPolicyStamp`: a machine-readable mark saying "this number came from
//! a rule Salt chose, and nobody has confirmed it yet".
//!
//! Salt already distinguishes statutory arithmetic from its own policy in
//! prose and in test names (`salt_policy_*` versus `statutory_*`, ADR-0008).
//! A trace an Operator reads on screen needs the same distinction as *data*,
//! not as a sentence baked into the calculator: the screen renders the
//! stamp, so the wording can change without the domain crate emitting
//! user-facing English, and a stamp can never be quietly dropped from a
//! figure that still depends on an unconfirmed rule.

use serde::{Deserialize, Serialize};

/// Which unconfirmed Salt rule a figure rests on. Named by the same
/// `SC-OPEN-n` reference `docs/domain/statutory-conformance.md` uses, so a
/// stamp on a payslip and a row in the conformance record are the same
/// identifier rather than two spellings of one idea.
///
/// Only the divisor is here today. The other open items either have no
/// per-figure trace to stamp (SC-OPEN-1, SC-OPEN-2) or are a refusal rather
/// than a policy (SC-OPEN-4); each gains a variant here when, and only when,
/// a figure it governs starts carrying workings of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SaltPolicyId {
    /// SC-OPEN-6 — the divisor converting a monthly salary to an hourly
    /// rate: `BasicPay x 12 / 52 / OrdinaryHours` (D18, ADR-0022). No
    /// published Namibian rule prescribing a divisor was found.
    SalaryToHourlyDivisor,
}

impl SaltPolicyId {
    /// The conformance record's own reference for this policy.
    pub fn reference(self) -> &'static str {
        match self {
            SaltPolicyId::SalaryToHourlyDivisor => "SC-OPEN-6",
        }
    }
}

/// How far the open question behind a policy has got. `NeedsConfirmation`
/// is the only state today, and it is deliberately explicit rather than
/// implied by the stamp's presence: a policy that is later confirmed keeps
/// its stamp and changes its status, so nothing has to be deleted from a
/// historical trace to record that the answer arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SaltPolicyStatus {
    /// Salt chose this rule. No regulator has confirmed it. It must never
    /// be described as law, and no test asserting it may be named
    /// `statutory_*` (ADR-0008).
    NeedsConfirmation,
}

/// The stamp itself, carried as data on every trace whose figure depends on
/// an unconfirmed Salt rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SaltPolicyStamp {
    pub id: SaltPolicyId,
    pub status: SaltPolicyStatus,
}

impl SaltPolicyStamp {
    /// SC-OPEN-6, `NEEDS CONFIRMATION`. A constructor rather than a `const`
    /// literal at every call site, so the divisor's status is stated in one
    /// place and changing it cannot leave one trace claiming otherwise.
    pub const SALARY_TO_HOURLY_DIVISOR: SaltPolicyStamp = SaltPolicyStamp {
        id: SaltPolicyId::SalaryToHourlyDivisor,
        status: SaltPolicyStatus::NeedsConfirmation,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_divisor_stamp_names_the_conformance_records_own_reference() {
        assert_eq!(
            SaltPolicyStamp::SALARY_TO_HOURLY_DIVISOR.id.reference(),
            "SC-OPEN-6"
        );
        assert_eq!(
            SaltPolicyStamp::SALARY_TO_HOURLY_DIVISOR.status,
            SaltPolicyStatus::NeedsConfirmation
        );
    }

    #[test]
    fn a_stamp_round_trips() {
        let stamp = SaltPolicyStamp::SALARY_TO_HOURLY_DIVISOR;
        let json = serde_json::to_string(&stamp).unwrap();

        assert_eq!(
            serde_json::from_str::<SaltPolicyStamp>(&json).unwrap(),
            stamp
        );
    }
}
