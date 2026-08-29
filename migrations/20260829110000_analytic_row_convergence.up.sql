-- The converged analytic line (the row-shape convergence this migration lands):
-- timesheet.timesheets becomes the ONE analytic row — logged time plus its
-- plain-stored money columns. Additive only: every new column carries a
-- default so the guarded write surface built against the previous row shape
-- keeps working un-re-pinned.
--
-- Plain-amount ruling (TSM-1): amounts and rates are STORED, never computed on
-- read. They are re-resolved ONLY on writes touching the rate-determining
-- fields [time_start, time_end, unit_amount, employee_id, activity_type_id];
-- a moved row keeps its snapshot. The service layer owns that trigger set —
-- the DB carries no repricing logic.
--
-- The row carries NO state and NO approval of its own: the per-employee/month
-- approval cycle (timesheet.timesheet_approvals) stays the billability gate.

-- 1. Enum extension FIRST and as its own statement: Postgres cannot add an enum
--    value and use it inside the same transaction, and nothing below uses it.
--    Leave-regenerated rows are neither work nor overtime; a third variant
--    keeps work-hours reads honest and makes the leave fence queryable
--    (entry_type = 'timeoff' AND source_timeoff_request_id IS NOT NULL).
--
--    The type is located THROUGH THE TABLE'S COLUMN (atttypid), not by a hardcoded
--    schema: the original enum-creation migration is unqualified, so where the type
--    landed depends on the runner's search_path at the time (observed both
--    `timesheet.timesheet_type` and `public.timesheet_type` in the wild). Resolving
--    via the column's own type oid is correct in either world.
DO $$
DECLARE
    col_type oid;
BEGIN
    SELECT a.atttypid INTO col_type
      FROM pg_attribute a
     WHERE a.attrelid = 'timesheet.timesheets'::regclass
       AND a.attname = 'entry_type';
    IF col_type IS NULL THEN
        RAISE EXCEPTION 'timesheet.timesheets.entry_type not found — cannot extend the enum';
    END IF;
    EXECUTE format('ALTER TYPE %s ADD VALUE IF NOT EXISTS ''timeoff''', col_type::regtype);
END
$$;

-- 2. The analytic-line columns (all additive, all defaulted).
ALTER TABLE timesheet.timesheets
    ADD COLUMN IF NOT EXISTS unit_amount NUMERIC(10,2) NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS currency VARCHAR(3) NOT NULL DEFAULT 'IDR',
    ADD COLUMN IF NOT EXISTS activity_type_id UUID,
    ADD COLUMN IF NOT EXISTS billing_rate NUMERIC(18,2),
    ADD COLUMN IF NOT EXISTS costing_rate NUMERIC(18,2),
    ADD COLUMN IF NOT EXISTS is_billable BOOLEAN NOT NULL DEFAULT TRUE,
    ADD COLUMN IF NOT EXISTS billable_amount NUMERIC(18,2) NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS costing_amount NUMERIC(18,2) NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS invoice_id UUID,
    ADD COLUMN IF NOT EXISTS source_timeoff_request_id UUID;

-- Non-negative money/hours CHECKs, matching the ecosystem's @non_negative
-- convention (amounts are stored POSITIVE — the analytic-ledger negative sign
-- is an artifact this row does not own). Guarded: Postgres has no
-- ADD CONSTRAINT IF NOT EXISTS.
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'timesheets_unit_amount_non_negative') THEN
        ALTER TABLE timesheet.timesheets ADD CONSTRAINT timesheets_unit_amount_non_negative CHECK (unit_amount >= 0);
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'timesheets_billable_amount_non_negative') THEN
        ALTER TABLE timesheet.timesheets ADD CONSTRAINT timesheets_billable_amount_non_negative CHECK (billable_amount >= 0);
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'timesheets_costing_amount_non_negative') THEN
        ALTER TABLE timesheet.timesheets ADD CONSTRAINT timesheets_costing_amount_non_negative CHECK (costing_amount >= 0);
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'timesheets_billing_rate_non_negative') THEN
        ALTER TABLE timesheet.timesheets ADD CONSTRAINT timesheets_billing_rate_non_negative CHECK (billing_rate IS NULL OR billing_rate >= 0);
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'timesheets_costing_rate_non_negative') THEN
        ALTER TABLE timesheet.timesheets ADD CONSTRAINT timesheets_costing_rate_non_negative CHECK (costing_rate IS NULL OR costing_rate >= 0);
    END IF;
END
$$;

-- 3. Backfill hours from the windows: rows that always carried explicit
--    start/end instants get their plain-stored unit_amount. Rows without
--    windows stay 0 — honest absence, documented; hours are not invented.
UPDATE timesheet.timesheets
   SET unit_amount = ROUND((EXTRACT(EPOCH FROM (time_end - time_start)) / 3600)::numeric, 2)
 WHERE time_start IS NOT NULL
   AND time_end IS NOT NULL
   AND unit_amount = 0;

-- 4. Rates/amounts stay NULL/0 on purpose: no rate source existed before this
--    migration, and a silently invented rate is worse than a visible NULL.

-- 5. Leave-regeneration no-duplicates backstop: regeneration is
--    delete-and-regenerate inside one tx; this partial unique is the DB-level
--    guarantee a raw duplicate insert cannot survive (ADR-0015 db enforcement).
CREATE UNIQUE INDEX IF NOT EXISTS uq_timesheets_leave_origin
    ON timesheet.timesheets (company_id, source_timeoff_request_id, date)
    WHERE source_timeoff_request_id IS NOT NULL
      AND (metadata->>'deleted_at') IS NULL;

-- 6. Read-path indexes: the billing/roll-up scan and the reversal lookup.
CREATE INDEX IF NOT EXISTS idx_timesheets_project_period
    ON timesheet.timesheets (project_id, employee_id, year, month)
    WHERE project_id IS NOT NULL
      AND (metadata->>'deleted_at') IS NULL;

CREATE INDEX IF NOT EXISTS idx_timesheets_invoice_id
    ON timesheet.timesheets (invoice_id)
    WHERE invoice_id IS NOT NULL;

-- 7. Invoiced-row write guard (DB backstop): once a row carries invoice_id it
--    may not change any pricing or anchoring column — corrections flow through
--    the reversal path, which CLEARS invoice_id first (clearing stays legal;
--    the WHEN clause only fires while the link is present on both sides).
--    metadata-only updates (soft delete) pass; the service layer refuses those
--    separately with a typed error.
CREATE OR REPLACE FUNCTION timesheet.timesheets_invoiced_row_guard() RETURNS trigger AS $$
BEGIN
    IF OLD.time_start     IS DISTINCT FROM NEW.time_start
    OR OLD.time_end       IS DISTINCT FROM NEW.time_end
    OR OLD.unit_amount    IS DISTINCT FROM NEW.unit_amount
    OR OLD.employee_id    IS DISTINCT FROM NEW.employee_id
    OR OLD.project_id     IS DISTINCT FROM NEW.project_id
    OR OLD.task_id        IS DISTINCT FROM NEW.task_id
    OR OLD.date           IS DISTINCT FROM NEW.date
    OR OLD.year           IS DISTINCT FROM NEW.year
    OR OLD.month          IS DISTINCT FROM NEW.month
    OR OLD.activity_type_id IS DISTINCT FROM NEW.activity_type_id
    OR OLD.is_billable    IS DISTINCT FROM NEW.is_billable
    OR OLD.billing_rate   IS DISTINCT FROM NEW.billing_rate
    OR OLD.costing_rate   IS DISTINCT FROM NEW.costing_rate
    OR OLD.billable_amount IS DISTINCT FROM NEW.billable_amount
    OR OLD.costing_amount IS DISTINCT FROM NEW.costing_amount
    THEN
        RAISE EXCEPTION 'timesheet_invoiced_row_locked: an invoiced analytic line cannot change its pricing or anchoring columns (clear the invoice link via the reversal path first) — row %', OLD.id;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS timesheets_invoiced_row_guard ON timesheet.timesheets;
CREATE TRIGGER timesheets_invoiced_row_guard
    BEFORE UPDATE ON timesheet.timesheets
    FOR EACH ROW
    WHEN (OLD.invoice_id IS NOT NULL AND NEW.invoice_id IS NOT NULL)
    EXECUTE FUNCTION timesheet.timesheets_invoiced_row_guard();
