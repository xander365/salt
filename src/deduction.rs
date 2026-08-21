//! `Deduction`: one classified amount withheld under a stated legal authority.

use serde::{Deserialize, Serialize};

use crate::money::Money;

/// One classified amount withheld from the employee's remuneration under a
/// stated legal authority. v1 constructs only the two statutory kinds; the
/// enum is open to further classifications so voluntary deductions are
/// additive later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Deduction {
    Statutory(StatutoryDeduction),
}

/// A deduction required by statute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StatutoryDeduction {
    PAYE(Money),
    SocialSecurity(Money),
}

impl Deduction {
    pub fn amount(self) -> Money {
        match self {
            Deduction::Statutory(StatutoryDeduction::PAYE(amount)) => amount,
            Deduction::Statutory(StatutoryDeduction::SocialSecurity(amount)) => amount,
        }
    }
}
