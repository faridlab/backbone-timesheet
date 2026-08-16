-- Reverse the seam columns and restore the plain period index.

DROP INDEX IF EXISTS timesheet.idx_timesheet_approvals_period_unique;

CREATE INDEX IF NOT EXISTS idx_timesheet_approvals_company_id_employee_id_year_month
    ON timesheet.timesheet_approvals (company_id, employee_id, year, month);

ALTER TABLE timesheet.timesheet_approvals DROP COLUMN IF EXISTS submitted_at;
ALTER TABLE timesheet.timesheet_approvals DROP COLUMN IF EXISTS approval_request_id;
