-- UnsupportedDeductionDeclaration (§4.5c): effective-dated, pinned to a
-- pay-period start date. No row in force at a period end means Unknown,
-- which the calculator refuses.
CREATE TABLE unsupported_deduction_declaration (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    employment_id  TEXT NOT NULL REFERENCES employment (id),
    effective_from DATE NOT NULL,
    status         TEXT NOT NULL CHECK (status IN ('confirmed_none', 'present')),
    kinds          JSONB,
    declared_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    declared_by    TEXT NOT NULL,
    UNIQUE (employment_id, effective_from),
    CHECK (
        (status = 'present' AND kinds IS NOT NULL AND jsonb_array_length(kinds) > 0)
        OR (status = 'confirmed_none' AND kinds IS NULL)
    )
);
