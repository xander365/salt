-- Employment is never physically deleted (§4.3); ending one is an end_date,
-- and a mis-created one is voided rather than removed.
CREATE TABLE employment (
    id          TEXT PRIMARY KEY,
    employer_id TEXT NOT NULL REFERENCES employer (id),
    person_id   TEXT NOT NULL,
    start_date  DATE NOT NULL,
    end_date    DATE,
    is_void     BOOLEAN NOT NULL DEFAULT FALSE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by  TEXT NOT NULL,
    CHECK (end_date IS NULL OR end_date >= start_date)
);
