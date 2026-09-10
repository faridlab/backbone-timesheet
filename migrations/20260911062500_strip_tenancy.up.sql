-- Hand-authored (user-owned). Not regenerated.
--
-- Strip every company-fence artifact from the timesheet tables (ADR-0029): the module is
-- tenant-agnostic; org scoping is installed by the COMPOSING service's tenancy decorator,
-- never by the module. Dropped here, per table: the company-leading indexes, the
-- <table>_company_isolation RLS policy, and the company_id column itself.
--
-- Tables: timesheets, timesheet_approvals.
--
-- Ordering guard (the decorator must run FIRST on any database with data): the module
-- never moves tenancy data. A table is safe to strip when EITHER
--   a) it carries org_unit_id with no NULLs — the decorator backfilled it from company_id —
--      or b) it is empty (a fresh database: the earlier chain files created it empty).
-- Otherwise the strip RAISEs, naming the decorator step, rather than dropping a column
-- that still holds the only tenancy key. The file is re-runnable (every drop is IF EXISTS
-- and the tracker has no checksums), so a failed run retries cleanly after the decorator
-- lands.
--
-- RLS enable/force flags are deliberately NOT touched: the decorator owns those now.
-- Three invariants are company-keyed only by posture, not by domain, and are restored here
-- in their tenant-free forms under the same names (an entry/approval/leave request belongs
-- to exactly one org unit under any deployment, so the keys need no tenant column):
--   - timesheets_no_overlap (the ranged-entry EXCLUDE) re-created without the
--     `company_id WITH =` leg — the employee + instant-range legs are the domain;
--   - uq_timesheets_leave_origin re-keyed to (source_timeoff_request_id, date) —
--     the leave-regeneration no-duplicates backstop;
--   - idx_timesheet_approvals_period_unique re-keyed to (employee_id, year, month) —
--     the one-cycle-per-employee-period rule the write service's period lock relies on.
-- No per-unit uniqueness posture is introduced here: under a composing decorator the
-- row-level fence scopes every one of these keys per unit already.

DO $$
DECLARE
    t text;
    has_org boolean;
    org_nulls bigint;
    total bigint;
    offenders text := '';
BEGIN
    FOREACH t IN ARRAY ARRAY['timesheets', 'timesheet_approvals']
    LOOP
        IF to_regclass(format('timesheet.%I', t)) IS NULL THEN
            CONTINUE; -- chain not fully applied on this database; nothing to strip
        END IF;

        SELECT EXISTS (
                   SELECT 1 FROM information_schema.columns
                   WHERE table_schema = 'timesheet' AND table_name = t AND column_name = 'org_unit_id'
               )
        INTO has_org;

        EXECUTE format('SELECT count(*) FROM timesheet.%I', t) INTO total;

        IF has_org THEN
            EXECUTE format(
                'SELECT count(*) FROM timesheet.%I WHERE org_unit_id IS NULL', t)
            INTO org_nulls;
        ELSE
            org_nulls := total; -- no org column: every row's only tenancy key is company_id
        END IF;

        IF has_org AND org_nulls = 0 THEN
            CONTINUE; -- decorator backfilled: safe
        END IF;
        IF total = 0 THEN
            CONTINUE; -- empty table (fresh database): safe
        END IF;
        offenders := offenders || format(' timesheet.%s (%s rows, %s rows not covered by org_unit_id);', t, total, org_nulls);
    END LOOP;

    IF offenders <> '' THEN
        RAISE EXCEPTION 'refusing to strip company_id — these tables are not yet covered by the tenancy decorator:%. Apply the composing service''s tenancy decorator (it backfills org_unit_id from company_id) and re-run; it is the only step that moves tenancy data.', offenders;
    END IF;
END $$;

-- ── timesheets ────────────────────────────────────────────────────────────────
-- The EXCLUDE names company_id, so it must go before the column does.
ALTER TABLE timesheet.timesheets DROP CONSTRAINT IF EXISTS timesheets_no_overlap;
DROP INDEX IF EXISTS timesheet.uq_timesheets_leave_origin;
DROP INDEX IF EXISTS timesheet.idx_timesheets_company_id_employee_id_date;
DROP POLICY IF EXISTS timesheets_company_isolation ON timesheet.timesheets;
ALTER TABLE timesheet.timesheets DROP COLUMN IF EXISTS company_id;

-- Restore the tenant-free invariants (same names, no tenant leg).
CREATE UNIQUE INDEX IF NOT EXISTS uq_timesheets_leave_origin
    ON timesheet.timesheets (source_timeoff_request_id, date)
    WHERE source_timeoff_request_id IS NOT NULL
      AND (metadata->>'deleted_at') IS NULL;

ALTER TABLE timesheet.timesheets
    ADD CONSTRAINT timesheets_no_overlap
    EXCLUDE USING gist (
        employee_id WITH =,
        tstzrange(time_start, time_end) WITH &&
    )
    WHERE (time_start IS NOT NULL AND time_end IS NOT NULL AND (metadata->>'deleted_at') IS NULL);

-- ── timesheet_approvals ───────────────────────────────────────────────────────
DROP INDEX IF EXISTS timesheet.idx_timesheet_approvals_period_unique;
DROP POLICY IF EXISTS timesheet_approvals_company_isolation ON timesheet.timesheet_approvals;
ALTER TABLE timesheet.timesheet_approvals DROP COLUMN IF EXISTS company_id;

CREATE UNIQUE INDEX IF NOT EXISTS idx_timesheet_approvals_period_unique
    ON timesheet.timesheet_approvals (employee_id, year, month)
    WHERE (metadata->>'deleted_at') IS NULL;
