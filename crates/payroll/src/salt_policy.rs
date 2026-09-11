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
/// The other open items either have no per-figure trace to stamp (SC-OPEN-1,
/// SC-OPEN-2) or are a refusal rather than a policy (SC-OPEN-4); each gains a
/// variant here when, and only when, a figure it governs starts carrying
/// workings of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SaltPolicyId {
    /// SC-OPEN-6 — the divisor converting a monthly salary to an hourly
    /// rate: `BasicPay x 12 / 52 / OrdinaryHours` (D18, ADR-0022). No
    /// published Namibian rule prescribing a divisor was found.
    SalaryToHourlyDivisor,
    /// SC-OPEN-7 — Salt grants no relief against taxable income for an
    /// employee's own medical aid premium (issue #78). NamRA's four allowed
    /// deductions do not list medical aid, which supports this reading, but
    /// no primary source confirming it was read.
    MedicalAidPremiumUnrelieved,
}

impl SaltPolicyId {
    /// The conformance record's own reference for this policy.
    pub fn reference(self) -> &'static str {
        match self {
            SaltPolicyId::SalaryToHourlyDivisor => "SC-OPEN-6",
            SaltPolicyId::MedicalAidPremiumUnrelieved => "SC-OPEN-7",
        }
    }
}

/// How far the open question behind a policy has got, and which authority is
/// being awaited. Deliberately explicit rather than implied by the stamp's
/// presence: a policy that is later confirmed keeps its stamp and changes its
/// status, so nothing has to be deleted from a historical trace to record
/// that the answer arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SaltPolicyStatus {
    /// Salt chose this rule; no regulator has confirmed it (SC-OPEN-6's own
    /// wording, `docs/domain/statutory-conformance.md`).
    NeedsConfirmation,
    /// Salt chose this reading of what NamRA requires; NamRA itself has not
    /// confirmed it (the wording SC-OPEN-1, SC-OPEN-4 and SC-OPEN-7 use).
    NeedsNamraConfirmation,
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

    /// SC-OPEN-7, `NEEDS NAMRA CONFIRMATION`: no relief against taxable
    /// income for the employee's own medical aid premium (issue #78).
    pub const MEDICAL_AID_PREMIUM_UNRELIEVED: SaltPolicyStamp = SaltPolicyStamp {
        id: SaltPolicyId::MedicalAidPremiumUnrelieved,
        status: SaltPolicyStatus::NeedsNamraConfirmation,
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
    fn the_medical_aid_stamp_names_the_conformance_records_own_reference() {
        assert_eq!(
            SaltPolicyStamp::MEDICAL_AID_PREMIUM_UNRELIEVED
                .id
                .reference(),
            "SC-OPEN-7"
        );
        assert_eq!(
            SaltPolicyStamp::MEDICAL_AID_PREMIUM_UNRELIEVED.status,
            SaltPolicyStatus::NeedsNamraConfirmation
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
