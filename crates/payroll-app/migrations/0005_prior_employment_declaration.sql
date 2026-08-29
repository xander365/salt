-- PriorEmploymentDeclaration (§4.5b): Employment + TaxYear state. Absence
-- means Unknown; the calculator itself refuses Unknown, so no gate is
-- needed here beyond one row per (employment, tax_year).
CREATE TABLE prior_employment_declaration (
    id                   UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    employment_id        TEXT NOT NULL REFERENCES employment (id),
    tax_year              INTEGER NOT NULL,
    status                TEXT NOT NULL CHECK (status IN ('confirmed_none', 'present')),
    taxable_remuneration  NUMERIC,
    paye                  NUMERIC,
    declared_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    declared_by           TEXT NOT NULL,
    UNIQUE (employment_id, tax_year),
    CHECK (
        (status = 'present' AND taxable_remuneration IS NOT NULL AND paye IS NOT NULL)
        OR (status = 'confirmed_none' AND taxable_remuneration IS NULL AND paye IS NULL)
    )
);
