-- Down: drop the declared rule constraints for timesheet module
ALTER TABLE IF EXISTS timesheet.timesheet_approvals DROP CONSTRAINT IF EXISTS ck_timesheet_approvals_remark_max_length__max_length;
ALTER TABLE IF EXISTS timesheet.timesheet_approvals DROP CONSTRAINT IF EXISTS ck_timesheet_approvals_employee_id_required__required;
DO $$
BEGIN
    IF to_regclass('timesheet.uq_timesheet_approvals_month__unique') IS NOT NULL AND to_regclass('timesheet.idx_timesheet_approvals_employee_id_year_month') IS NULL THEN
        ALTER INDEX timesheet.uq_timesheet_approvals_month__unique RENAME TO idx_timesheet_approvals_employee_id_year_month;
    END IF;
END $$;

ALTER TABLE IF EXISTS timesheet.timesheets DROP CONSTRAINT IF EXISTS ck_timesheets_costing_amount_non_negative__non_negative;
ALTER TABLE IF EXISTS timesheet.timesheets DROP CONSTRAINT IF EXISTS ck_timesheets_billable_amount_non_negative__non_negative;
ALTER TABLE IF EXISTS timesheet.timesheets DROP CONSTRAINT IF EXISTS ck_timesheets_costing_rate_non_negative__non_negative;
ALTER TABLE IF EXISTS timesheet.timesheets DROP CONSTRAINT IF EXISTS ck_timesheets_billing_rate_non_negative__non_negative;
ALTER TABLE IF EXISTS timesheet.timesheets DROP CONSTRAINT IF EXISTS ck_timesheets_currency_length__length;
ALTER TABLE IF EXISTS timesheet.timesheets DROP CONSTRAINT IF EXISTS ck_timesheets_currency_required__required;
ALTER TABLE IF EXISTS timesheet.timesheets DROP CONSTRAINT IF EXISTS ck_timesheets_unit_amount_non_negative__non_negative;
ALTER TABLE IF EXISTS timesheet.timesheets DROP CONSTRAINT IF EXISTS ck_timesheets_remark_max_length__max_length;
ALTER TABLE IF EXISTS timesheet.timesheets DROP CONSTRAINT IF EXISTS ck_timesheets_employee_id_required__required;
DO $$
BEGIN
    IF to_regclass('timesheet.uq_timesheets_date__unique') IS NOT NULL AND to_regclass('timesheet.idx_timesheets_source_timeoff_request_id_date') IS NULL THEN
        ALTER INDEX timesheet.uq_timesheets_date__unique RENAME TO idx_timesheets_source_timeoff_request_id_date;
    END IF;
END $$;

ALTER TABLE IF EXISTS timesheet.rate_cards DROP CONSTRAINT IF EXISTS ck_rate_cards_employee_id_required__required;
