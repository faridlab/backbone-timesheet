-- Reverse the analytic-line convergence: drop the write-guard trigger, the
-- read-path/leave-origin indexes, the CHECK constraints, and the added
-- columns. Rows written with the new columns lose their money snapshots and
-- leave origins (the pre-convergence row shape carried none of them).
--
-- NOTE: the 'timeoff' enum variant added by the up migration is NOT removed —
-- Postgres has no ALTER TYPE ... DROP VALUE, and rebuilding the type would
-- mean rewriting every row that carries it. The extra variant is inert on the
-- previous row shape (the write surface never mints it).

DROP TRIGGER IF EXISTS timesheets_invoiced_row_guard ON timesheet.timesheets;
DROP FUNCTION IF EXISTS timesheet.timesheets_invoiced_row_guard();

DROP INDEX IF EXISTS timesheet.idx_timesheets_invoice_id;
DROP INDEX IF EXISTS timesheet.idx_timesheets_project_period;
DROP INDEX IF EXISTS timesheet.uq_timesheets_leave_origin;

ALTER TABLE timesheet.timesheets
    DROP CONSTRAINT IF EXISTS timesheets_costing_rate_non_negative,
    DROP CONSTRAINT IF EXISTS timesheets_billing_rate_non_negative,
    DROP CONSTRAINT IF EXISTS timesheets_costing_amount_non_negative,
    DROP CONSTRAINT IF EXISTS timesheets_billable_amount_non_negative,
    DROP CONSTRAINT IF EXISTS timesheets_unit_amount_non_negative;

ALTER TABLE timesheet.timesheets
    DROP COLUMN IF EXISTS source_timeoff_request_id,
    DROP COLUMN IF EXISTS invoice_id,
    DROP COLUMN IF EXISTS costing_amount,
    DROP COLUMN IF EXISTS billable_amount,
    DROP COLUMN IF EXISTS is_billable,
    DROP COLUMN IF EXISTS costing_rate,
    DROP COLUMN IF EXISTS billing_rate,
    DROP COLUMN IF EXISTS activity_type_id,
    DROP COLUMN IF EXISTS currency,
    DROP COLUMN IF EXISTS unit_amount;
