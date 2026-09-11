//! `UnsupportedDeductionStatus`: what Salt knows about whether an Employee
//! has any fact that would change PAYE which Salt cannot calculate —
//! `docs/domain/statutory-conformance.md` §3.5, §5.5.
//!
//! Widened by issue #78 from its original meaning — "one of the four current
//! deductions NamRA's brochure allows against taxable income" — because
//! employer-paid medical aid is unsupported for a different reason: it is
//! not one of those four deductions at all, but a fringe benefit whose
//! taxable value Salt cannot compute (`Q-OPEN-9`). What every kind here
//! genuinely has in common is only this: each is a fact Salt has no rule to
//! turn into a PAYE figure, so `calculate` cannot proceed while any is
//! present.
//!
//! Named apart from `StatutoryDeduction` (`crate::deduction`), which means
//! a PAYE or social security amount actually withheld — a different
//! concept entirely, and this type must never be confused with it.

use serde::{Deserialize, Serialize};

/// One fact that would change PAYE which Salt cannot calculate
/// (`docs/domain/statutory-conformance.md` §3.5). The first four are
/// deductions NamRA's brochure allows against taxable income; the fifth,
/// `EmployerPaidMedicalAid`, is not one of those four at all but a fringe
/// benefit whose taxable value is unresolved (`Q-OPEN-9`) — grouped here
/// because both kinds block `calculate` the same way, not because both are
/// deductions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UnsupportedDeductionKind {
    ApprovedPensionFund,
    ProvidentFund,
    RetirementAnnuityFund,
    EducationPolicy,
    /// A benefit the Employer pays toward the Employee's medical aid, as
    /// distinct from the Employee's own premium — a `VoluntaryDeduction`,
    /// which Salt does support (issue #78). How the benefit is valued for
    /// tax is unresolved (`Q-OPEN-9`), so it is refused by name rather than
    /// guessed at.
    EmployerPaidMedicalAid,
}

impl std::fmt::Display for UnsupportedDeductionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            UnsupportedDeductionKind::ApprovedPensionFund => "approved pension fund contribution",
            UnsupportedDeductionKind::ProvidentFund => "provident fund contribution",
            UnsupportedDeductionKind::RetirementAnnuityFund => {
                "retirement annuity fund contribution"
            }
            UnsupportedDeductionKind::EducationPolicy => "education policy premium",
            UnsupportedDeductionKind::EmployerPaidMedicalAid => "employer-paid medical aid benefit",
        };
        f.write_str(name)
    }
}

/// A non-empty set of `UnsupportedDeductionKind`s an Employee has. Built
/// only through `new`, which rejects an empty `Vec` and collapses repeated
/// kinds while preserving their first-seen order. An empty collection would
/// be indistinguishable from `UnsupportedDeductionStatus::ConfirmedNone`, the
/// exact ambiguity this type exists to remove
/// (`docs/domain/statutory-conformance.md` §3.5).
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
            let mut unique_kinds = Vec::with_capacity(kinds.len());
            for kind in kinds {
                if !unique_kinds.contains(&kind) {
                    unique_kinds.push(kind);
                }
            }
            Ok(UnsupportedDeductionKinds(unique_kinds))
        }
    }

    pub fn as_slice(&self) -> &[UnsupportedDeductionKind] {
        &self.0
    }
}

/// Human-friendly only. The domain contract is the type, never this string
/// — a caller branches on `UnsupportedDeductionKind`, never on prose.
impl std::fmt::Display for UnsupportedDeductionKinds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (index, kind) in self.0.iter().enumerate() {
            if index > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{kind}")?;
        }
        Ok(())
    }
}

impl<'a> IntoIterator for &'a UnsupportedDeductionKinds {
    type Item = &'a UnsupportedDeductionKind;
    type IntoIter = std::slice::Iter<'a, UnsupportedDeductionKind>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
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

/// What Salt knows about whether an Employee has any fact that would change
/// PAYE which Salt cannot calculate. The three states are distinct on
/// purpose: nothing here can be mistaken for "nobody asked"
/// (`docs/domain/statutory-conformance.md` §5.5).
///
/// `Option<Vec<UnsupportedDeductionKind>>` and a bare `Vec` are both
/// rejected as the representation: either lets an empty collection mean
/// two different things at once, which is the ambiguity this type exists
/// to remove.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnsupportedDeductionStatus {
    /// Established: the Employee has none of the unsupported kinds.
    /// `calculate` proceeds normally.
    ConfirmedNone,
    /// The Employee has one or more of the unsupported kinds. `calculate`
    /// refuses, naming every kind present.
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

    /// Issue #78: employer-paid medical aid is refused by name before an
    /// Operator invests in setup, exactly like the original four kinds.
    #[test]
    fn employer_paid_medical_aid_is_a_named_unsupported_kind() {
        let kinds =
            UnsupportedDeductionKinds::new(vec![UnsupportedDeductionKind::EmployerPaidMedicalAid])
                .unwrap();
        assert_eq!(
            kinds.as_slice(),
            &[UnsupportedDeductionKind::EmployerPaidMedicalAid]
        );
        assert_eq!(
            UnsupportedDeductionKind::EmployerPaidMedicalAid.to_string(),
            "employer-paid medical aid benefit"
        );
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
    fn new_deduplicates_repeated_kinds_preserving_first_seen_order() {
        let kinds = UnsupportedDeductionKinds::new(vec![
            UnsupportedDeductionKind::EducationPolicy,
            UnsupportedDeductionKind::ApprovedPensionFund,
            UnsupportedDeductionKind::EducationPolicy,
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
    fn status_deserialize_rejects_a_present_with_no_kinds() {
        let result: Result<UnsupportedDeductionStatus, _> =
            serde_json::from_str(r#"{"Present":[]}"#);
        assert!(result.is_err());
    }

    #[test]
    fn status_round_trips_through_all_three_states() {
        let statuses = [
            UnsupportedDeductionStatus::ConfirmedNone,
            UnsupportedDeductionStatus::Unknown,
            UnsupportedDeductionStatus::Present(
                UnsupportedDeductionKinds::new(vec![
                    UnsupportedDeductionKind::ApprovedPensionFund,
                    UnsupportedDeductionKind::ProvidentFund,
                ])
                .unwrap(),
            ),
        ];
        for status in statuses {
            let json = serde_json::to_string(&status).unwrap();
            let round_tripped: UnsupportedDeductionStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(status, round_tripped);
        }
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
