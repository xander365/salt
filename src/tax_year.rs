//! `TaxYear`: the Namibian tax year, 1 March to end of February.

use serde::{Deserialize, Serialize};

/// The Namibian tax year running from 1 March of `starting_year` to the
/// last day of February the following year. Identified by the calendar
/// year it starts in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TaxYear(i32);

impl TaxYear {
    pub fn starting(starting_year: i32) -> Self {
        TaxYear(starting_year)
    }

    pub fn starting_year(self) -> i32 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starting_year_round_trips() {
        assert_eq!(TaxYear::starting(2026).starting_year(), 2026);
    }

    #[test]
    fn deserialize_round_trips() {
        let tax_year = TaxYear::starting(2026);
        let json = serde_json::to_string(&tax_year).unwrap();
        assert_eq!(serde_json::from_str::<TaxYear>(&json).unwrap(), tax_year);
    }
}
