-- Down: drop enum types for timesheet module
DROP TYPE IF EXISTS timesheet_approval_status CASCADE;
DROP TYPE IF EXISTS timesheet_type CASCADE;
