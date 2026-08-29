-- §4.3: an Employment is voided rather than deleted, and a voided
-- Employment never appears in run membership. A compound foreign key makes
-- that rule structural and concurrency-safe in both directions: a member
-- must reference an Employment whose `is_void` is false, so PostgreSQL also
-- refuses an attempt to void an Employment that is already a member.
--
-- `membership_employment_is_void` is deliberately a constant false marker,
-- rather than a second independently editable statement about an Employment.
-- Its CHECK means it can only ever select the non-void side of the foreign
-- key; the referenced row remains the source of truth.
ALTER TABLE employment
    ADD CONSTRAINT employment_id_is_void_unique UNIQUE (id, is_void);

ALTER TABLE payroll_run_employment
    ADD COLUMN membership_employment_is_void BOOLEAN NOT NULL DEFAULT FALSE,
    ADD CONSTRAINT membership_employment_is_not_void
        CHECK (membership_employment_is_void = FALSE),
    ADD CONSTRAINT membership_employment_is_active
        FOREIGN KEY (employment_id, membership_employment_is_void)
        REFERENCES employment (id, is_void);
