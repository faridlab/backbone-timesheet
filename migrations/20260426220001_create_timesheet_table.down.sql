-- Down: drop timesheet.timesheets table
DROP TABLE IF EXISTS timesheet.timesheets CASCADE;
DROP FUNCTION IF EXISTS timesheet.timesheets_audit_timestamp() CASCADE;
