-- Hand-authored (user-owned). Not regenerated.
--
-- Best-effort restore sketch for the tenancy strip (ADR-0029). This is a breaking module
-- release against dev-stage databases: the down re-adds the company_id column as nullable
-- with the company isolation policy shape and the company-keyed invariant forms, but restores
-- NO data — rows written after the strip (or after the decorator re-keyed them) carry
-- org_unit_id only. The composing service's tenancy decorator remains the live fence; treat
-- this down as a schema-shape sketch for archaeology, not a usable rollback.

ALTER TABLE timesheet.timesheets         ADD COLUMN IF NOT EXISTS company_id uuid;
ALTER TABLE timesheet.timesheet_approvals ADD COLUMN IF NOT EXISTS company_id uuid;

-- The strip's restored tenant-free invariants go away again (the company-keyed variants
-- would need company data this sketch does not restore).
DROP INDEX IF EXISTS timesheet.uq_timesheets_leave_origin;
DROP INDEX IF EXISTS timesheet.idx_timesheet_approvals_period_unique;
ALTER TABLE timesheet.timesheets DROP CONSTRAINT IF EXISTS timesheets_no_overlap;

CREATE INDEX IF NOT EXISTS idx_timesheets_company_id_employee_id_date
    ON timesheet.timesheets (company_id, employee_id, date);

CREATE UNIQUE INDEX IF NOT EXISTS uq_timesheets_leave_origin
    ON timesheet.timesheets (company_id, source_timeoff_request_id, date)
    WHERE source_timeoff_request_id IS NOT NULL
      AND (metadata->>'deleted_at') IS NULL;

ALTER TABLE timesheet.timesheets
    ADD CONSTRAINT timesheets_no_overlap
    EXCLUDE USING gist (
        company_id WITH =,
        employee_id WITH =,
        tstzrange(time_start, time_end) WITH &&
    )
    WHERE (time_start IS NOT NULL AND time_end IS NOT NULL AND (metadata->>'deleted_at') IS NULL);

CREATE UNIQUE INDEX IF NOT EXISTS idx_timesheet_approvals_period_unique
    ON timesheet.timesheet_approvals (company_id, employee_id, year, month)
    WHERE (metadata->>'deleted_at') IS NULL;

CREATE POLICY timesheets_company_isolation ON timesheet.timesheets
    FOR ALL
    USING      (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);
CREATE POLICY timesheet_approvals_company_isolation ON timesheet.timesheet_approvals
    FOR ALL
    USING      (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);
