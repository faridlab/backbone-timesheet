ALTER TABLE timesheet.timesheets DROP COLUMN IF EXISTS row_status;
DROP TYPE IF EXISTS timesheet_row_status;
