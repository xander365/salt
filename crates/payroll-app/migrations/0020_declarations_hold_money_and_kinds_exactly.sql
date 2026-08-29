-- Two facts the declaration tables (§4.5b, §4.5c) stated only by convention.
--
-- 1. `PriorEmploymentFigures` carries two `Money` amounts, and a `Money` is a
--    whole, non-negative number of cents. NUMERIC admits a fraction of a cent
--    and a negative, and no reader can decode it back into the `i64`
--    `Money::from_cents` takes -- every existing read already casts `::bigint`
--    on the way out. BIGINT is what the writer binds (`Money::cents()`), what
--    the reader takes back, and what `compensation_terms.basic_pay`, the
--    schema's other Money column, already is. The CHECK makes the reader's
--    `Money::from_cents` a schema guarantee rather than a hope about every
--    future writer, exactly as 0019 did for `basic_pay`.
--
-- 2. `unsupported_deduction_declaration.kinds` is the serialized form of
--    `UnsupportedDeductionKinds`, which is always a non-empty JSON array.
--    `jsonb_array_length` raises an error rather than returning false when
--    handed a JSON object or scalar, so the original CHECK could not refuse
--    one -- it aborted evaluating instead. Testing the type first, in a CASE
--    so the order is the SQL standard's rather than the planner's, turns that
--    abort into the refusal it was always meant to be.

ALTER TABLE prior_employment_declaration
    ALTER COLUMN taxable_remuneration TYPE BIGINT USING taxable_remuneration::bigint,
    ALTER COLUMN paye TYPE BIGINT USING paye::bigint;

ALTER TABLE prior_employment_declaration
    ADD CONSTRAINT prior_employment_declaration_figures_are_money
    CHECK (
        (taxable_remuneration IS NULL OR taxable_remuneration >= 0)
        AND (paye IS NULL OR paye >= 0)
    );

ALTER TABLE unsupported_deduction_declaration
    DROP CONSTRAINT unsupported_deduction_declaration_check;

ALTER TABLE unsupported_deduction_declaration
    ADD CONSTRAINT unsupported_deduction_declaration_names_kinds_when_present
    CHECK (
        CASE status
            WHEN 'present' THEN
                CASE WHEN jsonb_typeof(kinds) = 'array'
                     THEN jsonb_array_length(kinds) > 0
                     ELSE FALSE
                END
            ELSE kinds IS NULL
        END
    );
