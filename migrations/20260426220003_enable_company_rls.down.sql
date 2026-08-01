-- Down: remove the company RLS fence for timesheet module

-- Reverse the company RLS fence for timesheet.timesheets
DROP POLICY IF EXISTS timesheets_company_isolation ON timesheet.timesheets;
ALTER TABLE timesheet.timesheets NO FORCE ROW LEVEL SECURITY;
ALTER TABLE timesheet.timesheets DISABLE ROW LEVEL SECURITY;

-- Reverse the company RLS fence for timesheet.timesheet_approvals
DROP POLICY IF EXISTS timesheet_approvals_company_isolation ON timesheet.timesheet_approvals;
ALTER TABLE timesheet.timesheet_approvals NO FORCE ROW LEVEL SECURITY;
ALTER TABLE timesheet.timesheet_approvals DISABLE ROW LEVEL SECURITY;

