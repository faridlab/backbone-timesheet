-- Down: drop timesheet.timesheet_approvals table
DROP TABLE IF EXISTS timesheet.timesheet_approvals CASCADE;
DROP FUNCTION IF EXISTS timesheet.timesheet_approvals_audit_timestamp() CASCADE;
