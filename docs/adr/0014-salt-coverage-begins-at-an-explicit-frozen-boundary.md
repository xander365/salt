# Salt's coverage of an Employment begins at an explicit, frozen boundary

An `OpeningBalance` carries `first_salt_period_end` — the first PayPeriod end date Salt is responsible for, for that Employment in that TaxYear — alongside the prior figures. Every period of the TaxYear ending before it is pre-Salt and accounted for inside those figures. The row is never auto-created, and the whole row freezes at that Employment's first finalization in the TaxYear. We rejected auto-creating a zero balance when an Employment is created, and rejected recording coverage as "the last period covered".

**Auto-creating a zero row was the serious error.** It turned "nobody entered prior year-to-date" into "confirmed zero" — the precise conflation INV-012 exists to prevent and that `PriorEmployment::Unknown` was built to prevent inside the calculator — committed by Salt itself rather than by a user. An `OpeningBalance` is an affirmative payroll fact or it is nothing.

**The boundary exists to separate two cases that otherwise look identical.** An employer adopts Salt in October, and their `OpeningBalance` legitimately covers March through September: Salt must not demand a September `FinalizedPayroll`, because September was never Salt's. An employer who adopted in March, ran through August and simply forgot September must be refused. Both look like "October is finalizing and there is no September payroll".

The distinction is not made by the wording of the boundary — `through_period_end` and `first_salt_period_end` carry identical information. It is made by the boundary being **set once and frozen**. In the adoption case the boundary reads 31-October and September resolves. In the forgotten case the boundary froze at 31-March when March finalized, so it cannot retroactively be moved to October, and the run refuses. Freezing is what does the work; `first_salt_period_end` is chosen over `through_period_end` only because it states the answer to the question the sequencing rule actually asks.

**A row is required exactly when there are pre-Salt periods to account for**, and that is decided by the sequencing walk-back rather than by a rule of its own. Absence of an `OpeningBalance` is legal only when Salt's own live history, a reasoned removal, or the Employment not yet existing already resolves every earlier period of the TaxYear — a positive fact Salt checks, never a silence Salt assumes. Requiring a row unconditionally would mean eight auto-clicked "confirm zero" checkboxes every March for an eight-person employer, which is the ceremony ADR-0010 rejected in `Reviewed`.

## Consequences

- Four guards are checked when the row is written: the boundary must be a period end the Employer's `PaySchedule` generates; it must fall inside the row's TaxYear; it must be on or after the Employment's first payable period end in that TaxYear; and non-zero prior figures over an empty covered span are refused. Zero figures over a non-empty span are allowed — unpaid leave and nil-PAYE months are ordinary.
- The old finalization step "verify every member has an `OpeningBalance` for the TaxYear" is deleted. It was the auto-created row's partner, and the walk-back replaces it.
- A continuing employee needs no row at all in a TaxYear Salt ran end to end.
- The boundary is per Employment, not per Employer: employees hired after adoption have later boundaries and empty covered spans.
