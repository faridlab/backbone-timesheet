-- Approval seam + period uniqueness (Wave 1 P2, pillar-people H-6):
-- approval_request_id is the logical link to approvals.ApprovalRequest
-- (mirrors timeoff's P1 ApprovalFiling seam — one cycle per employee per
-- period, enforced UNIQUE among live rows); submitted_at stamps the
-- validation-window check at submit time.

ALTER TABLE timesheet.timesheet_approvals ADD COLUMN IF NOT EXISTS approval_request_id UUID;
ALTER TABLE timesheet.timesheet_approvals ADD COLUMN IF NOT EXISTS submitted_at TIMESTAMPTZ;

DROP INDEX IF EXISTS timesheet.idx_timesheet_approvals_company_id_employee_id_year_month;

CREATE UNIQUE INDEX IF NOT EXISTS idx_timesheet_approvals_period_unique
    ON timesheet.timesheet_approvals (company_id, employee_id, year, month)
    WHERE (metadata->>'deleted_at') IS NULL;
