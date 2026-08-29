-- Employer, and its mutable PaySchedule (docs/domain/payroll-run-persistence.md §4.2).
-- Whether a PaySchedule change is legal (never mid-tax-year) is a domain
-- question Rust decides; this table only stores the current schedule.
CREATE TABLE employer (
    id                   TEXT PRIMARY KEY,
    period_end_day_kind  TEXT NOT NULL CHECK (period_end_day_kind IN ('day', 'last_day_of_month')),
    period_end_day_value SMALLINT CHECK (period_end_day_value BETWEEN 1 AND 28),
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by           TEXT NOT NULL,
    CHECK (
        (period_end_day_kind = 'day' AND period_end_day_value IS NOT NULL)
        OR (period_end_day_kind = 'last_day_of_month' AND period_end_day_value IS NULL)
    )
);
