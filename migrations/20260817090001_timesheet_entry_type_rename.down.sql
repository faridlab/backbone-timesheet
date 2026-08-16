-- Reverse the entry_type rename.

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_schema = 'timesheet'
          AND table_name = 'timesheets'
          AND column_name = 'entry_type'
    ) THEN
        ALTER TABLE timesheet.timesheets RENAME COLUMN entry_type TO "type";
    END IF;
END
$$;
