-- Ordinary hours are an explicit assumption for a future derived hourly rate.
-- Historical CompensationTerms do not tell us what their BasicPay covered, so
-- this column is nullable and deliberately has no backfill.
ALTER TABLE compensation_terms
    ADD COLUMN ordinary_hours NUMERIC(5, 2);

ALTER TABLE compensation_terms
    ADD CONSTRAINT compensation_terms_ordinary_hours_are_weekly_and_positive
    CHECK (ordinary_hours IS NULL OR (ordinary_hours > 0 AND ordinary_hours <= 168));
