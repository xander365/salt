-- CompensationTerms stays correctable master data forever (§6.5, decision 30);
-- nothing here freezes it. effective_until is nullable: a row is in force
-- until the next row's effective_from (§4.4).
CREATE TABLE compensation_terms (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    employment_id   TEXT NOT NULL REFERENCES employment (id),
    effective_from  DATE NOT NULL,
    effective_until DATE,
    basic_pay       BIGINT NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by      TEXT NOT NULL,
    UNIQUE (employment_id, effective_from),
    CHECK (effective_until IS NULL OR effective_until >= effective_from)
);
