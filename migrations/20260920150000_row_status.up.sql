-- The per-row review state: partial approval without sending back the
-- whole month. Rows default draft (the period stays the coarse grain);
-- a manager's one-day verdict moves one row to approved or returned, and a
-- RETURNED row is editable while its period sits under review.

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'timesheet_row_status') THEN
        CREATE TYPE timesheet_row_status AS ENUM ('draft', 'approved', 'returned');
    END IF;
END
$$;

ALTER TABLE timesheet.timesheets
    ADD COLUMN IF NOT EXISTS row_status timesheet_row_status NOT NULL DEFAULT 'draft';

-- Existing rows in approved periods carry their period's verdict.
UPDATE timesheet.timesheets t
   SET row_status = 'approved'
  FROM timesheet.timesheet_approvals a
 WHERE a.employee_id = t.employee_id
   AND a.year = t.year AND a.month = t.month
   AND a.status = 'approved';
