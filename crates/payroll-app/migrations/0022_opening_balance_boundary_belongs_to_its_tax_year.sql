-- Guard 2 of §4.5 -- `first_salt_period_end` must fall inside the row's own
-- `TaxYear` -- was stated only in Rust. Every later reader of this table
-- joins it by `(employment_id, tax_year)` and then treats
-- `first_salt_period_end` as a date inside that year: §7's walk-back asks
-- "is this period before the boundary" of a boundary it never re-dates, and
-- §8 adds these figures to one TaxYear's year-to-date. A row whose boundary
-- belongs to a different TaxYear would answer both questions wrongly and
-- silently.
--
-- ADR-0005 keys a `PayPeriod`'s TaxYear on its **end** date alone, so
-- January and February belong to the year that started the previous March.
-- That is `TaxYear::for_period_end`, restated here as the CHECK so the
-- agreement is a schema guarantee rather than a hope about every writer --
-- the same move 0019 made for `basic_pay` and 0021 for this table's Money
-- columns. Existing rows that disagree make this migration stop, which is
-- what should happen: an operator must decide which of the two figures is
-- the true one.

ALTER TABLE opening_balance
    ADD CONSTRAINT opening_balance_boundary_belongs_to_its_tax_year
    CHECK (
        tax_year = CASE
            WHEN EXTRACT(MONTH FROM first_salt_period_end) >= 3
                THEN EXTRACT(YEAR FROM first_salt_period_end)::int
            ELSE EXTRACT(YEAR FROM first_salt_period_end)::int - 1
        END
    );
