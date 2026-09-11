//! `Deduction`: one classified amount withheld under a stated legal authority
//! or under the employee's own instruction.

use serde::{Deserialize, Serialize};

use crate::money::Money;
use crate::salt_policy::SaltPolicyStamp;

/// One classified amount withheld from the employee's remuneration: either
/// required by statute, or a voluntary deduction the employee has authorized
/// (issue #78). Exactly one kind of each exists today; a further kind of
/// either is a code change and a deliberate decision, never a free-text
/// "other deduction" type (D7 — medical aid is the driving example, not
/// unrestricted licence for any deduction).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Deduction {
    Statutory(StatutoryDeduction),
    Voluntary(VoluntaryDeduction),
}

/// A deduction required by statute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StatutoryDeduction {
    PAYE(Money),
    SocialSecurity(Money),
}

/// A deduction the employee has authorized against their own pay, computed
/// with no arithmetic of its own — the amount is exactly the amount
/// instructed (`VoluntaryDeductionInstruction`).
///
/// `policy` carries the `SC-OPEN-7` stamp: Salt grants this deduction no
/// relief against taxable income, which is Salt's own reading and
/// `NEEDS NAMRA CONFIRMATION`, never a fact this crate may present as
/// settled law.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VoluntaryDeduction {
    MedicalAidPremium {
        amount: Money,
        policy: SaltPolicyStamp,
    },
}

/// One voluntary deduction instruction supplied to the calculator —
/// `VoluntaryDeduction`'s counterpart to `EarningInstruction`. Carries no
/// policy stamp of its own: the stamp is `calculate`'s own output, attached
/// once, the same split `EarningInstruction`/`Earning` already draws for
/// overtime's trace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VoluntaryDeductionInstruction {
    MedicalAidPremium(Money),
}

impl Deduction {
    pub fn amount(self) -> Money {
        match self {
            Deduction::Statutory(StatutoryDeduction::PAYE(amount)) => amount,
            Deduction::Statutory(StatutoryDeduction::SocialSecurity(amount)) => amount,
            Deduction::Voluntary(VoluntaryDeduction::MedicalAidPremium { amount, .. }) => amount,
        }
    }
}
