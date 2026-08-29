-- `finalized_payroll.tax_year` must agree with `period_end`, the same
-- agreement 0022 already demanded of `opening_balance`.
--
-- §8's year-to-date read is the reader that makes this load-bearing. It cuts
-- history by date -- `live.period_end < $this_period_end` -- and then filters
-- by `finalized.tax_year`, so the two columns answer one question together. A
-- row whose `tax_year` disagreed with its own `period_end` would be dropped
-- from the TaxYear it belongs to, or summed into one it does not, and either
-- way a cumulative PAYE figure would be wrong for the rest of the year with
-- nothing on screen to show it. `finalized_payroll` has no UPDATE grant, so
-- such a row could never be repaired in place either.
--
-- ADR-0005 keys a `PayPeriod`'s TaxYear on its **end** date alone, so January
-- and February belong to the year that started the previous March. That is
-- `TaxYear::for_period_end`, restated here as the CHECK so the agreement is a
-- schema guarantee rather than a hope about every writer -- exactly the move
-- 0022 made for the boundary date on `opening_balance`. Existing rows that
-- disagree make this migration stop, which is what should happen: an operator
-- must decide which of the two figures states the truth.

ALTER TABLE finalized_payroll
    ADD CONSTRAINT finalized_payroll_belongs_to_its_tax_year
    CHECK (
        tax_year = CASE
            WHEN EXTRACT(MONTH FROM period_end) >= 3
                THEN EXTRACT(YEAR FROM period_end)::int
            ELSE EXTRACT(YEAR FROM period_end)::int - 1
        END
    );
