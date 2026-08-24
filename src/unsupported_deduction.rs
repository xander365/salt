//! `UnsupportedDeductionStatus`: what Salt knows about whether an Employee
//! has any of the four deduction kinds Salt v1 does not support —
//! `docs/domain/statutory-conformance.md` §3.5, §5.5.
//!
//! Named apart from `StatutoryDeduction` (`crate::deduction`), which means
//! a PAYE or social security amount actually withheld — a different
//! concept entirely, and this type must never be confused with it.

use serde::{Deserialize, Serialize};

/// One of the four current deductions NamRA's brochure allows against
/// taxable income (§3.5), none of which Salt v1 calculates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UnsupportedDeductionKind {
    ApprovedPensionFund,
    ProvidentFund,
    RetirementAnnuityFund,
    EducationPolicy,
}

/// A non-empty set of `UnsupportedDeductionKind`s an Employee has. Built
/// only through `new`, which rejects an empty `Vec` — an empty collection
/// would be indistinguishable from `UnsupportedDeductionStatus::ConfirmedNone`,
/// the exact ambiguity this type exists to remove (§3.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<UnsupportedDeductionKind>")]
#[serde(into = "Vec<UnsupportedDeductionKind>")]
pub struct UnsupportedDeductionKinds(Vec<UnsupportedDeductionKind>);

/// `UnsupportedDeductionKinds::new` was given an empty `Vec`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmptyUnsupportedDeductionKinds;

impl std::fmt::Display for EmptyUnsupportedDeductionKinds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "UnsupportedDeductionKinds requires at least one kind; an empty collection is ambiguous with ConfirmedNone"
        )
    }
}

impl std::error::Error for EmptyUnsupportedDeductionKinds {}

impl UnsupportedDeductionKinds {
    pub fn new(
        kinds: Vec<UnsupportedDeductionKind>,
    ) -> Result<Self, EmptyUnsupportedDeductionKinds> {
        if kinds.is_empty() {
            Err(EmptyUnsupportedDeductionKinds)
        } else {
            Ok(UnsupportedDeductionKinds(kinds))
        }
    }

    pub fn as_slice(&self) -> &[UnsupportedDeductionKind] {
        &self.0
    }
}

impl TryFrom<Vec<UnsupportedDeductionKind>> for UnsupportedDeductionKinds {
    type Error = EmptyUnsupportedDeductionKinds;

    fn try_from(kinds: Vec<UnsupportedDeductionKind>) -> Result<Self, Self::Error> {
        UnsupportedDeductionKinds::new(kinds)
    }
}

impl From<UnsupportedDeductionKinds> for Vec<UnsupportedDeductionKind> {
    fn from(kinds: UnsupportedDeductionKinds) -> Vec<UnsupportedDeductionKind> {
        kinds.0
    }
}

/// What Salt knows about whether an Employee has any of the four
/// deduction kinds it does not support. The three states are distinct on
/// purpose: nothing here can be mistaken for "nobody asked" (§5.5).
///
/// `Option<Vec<UnsupportedDeductionKind>>` and a bare `Vec` are both
/// rejected as the representation: either lets an empty collection mean
/// two different things at once, which is the ambiguity this type exists
/// to remove.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnsupportedDeductionStatus {
    /// Established: the Employee has none of the four kinds. `calculate`
    /// proceeds normally.
    ConfirmedNone,
    /// The Employee has one or more of the four kinds. `calculate` refuses,
    /// naming every kind present.
    Present(UnsupportedDeductionKinds),
    /// Nobody has established the fact. `calculate` refuses rather than
    /// treat an unasked question as a confirmed "no".
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_rejects_an_empty_collection() {
        assert_eq!(
            UnsupportedDeductionKinds::new(Vec::new()),
            Err(EmptyUnsupportedDeductionKinds)
        );
    }

    #[test]
    fn new_accepts_one_kind() {
        let kinds =
            UnsupportedDeductionKinds::new(vec![UnsupportedDeductionKind::ProvidentFund]).unwrap();
        assert_eq!(kinds.as_slice(), &[UnsupportedDeductionKind::ProvidentFund]);
    }

    #[test]
    fn new_preserves_every_kind_and_its_order() {
        let kinds = UnsupportedDeductionKinds::new(vec![
            UnsupportedDeductionKind::EducationPolicy,
            UnsupportedDeductionKind::ApprovedPensionFund,
        ])
        .unwrap();
        assert_eq!(
            kinds.as_slice(),
            &[
                UnsupportedDeductionKind::EducationPolicy,
                UnsupportedDeductionKind::ApprovedPensionFund,
            ]
        );
    }

    #[test]
    fn deserialize_rejects_an_empty_collection() {
        let result: Result<UnsupportedDeductionKinds, _> = serde_json::from_str("[]");
        assert!(result.is_err());
    }

    #[test]
    fn deserialize_round_trips() {
        let kinds =
            UnsupportedDeductionKinds::new(vec![UnsupportedDeductionKind::RetirementAnnuityFund])
                .unwrap();
        let json = serde_json::to_string(&kinds).unwrap();
        let round_tripped: UnsupportedDeductionKinds = serde_json::from_str(&json).unwrap();
        assert_eq!(kinds, round_tripped);
    }
}
