-- Reverse the overlap invariant. btree_gist stays installed (shared extension).

ALTER TABLE timesheet.timesheets
    DROP CONSTRAINT IF EXISTS timesheets_no_overlap;
