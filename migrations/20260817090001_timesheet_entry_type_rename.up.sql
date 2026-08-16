-- Fix the latent type/entry_type split (Wave 1 P2): the schema YAML renamed the
-- field `type` → `entry_type` (the codegen cannot escape the `type` keyword) but
-- no migration followed, so the generated entity/repository speak `entry_type`
-- while the DB column is still `type`. Module has never been composed, so no
-- data considerations — a guarded rename closes the gap.

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_schema = 'timesheet'
          AND table_name = 'timesheets'
          AND column_name = 'type'
    ) THEN
        ALTER TABLE timesheet.timesheets RENAME COLUMN "type" TO entry_type;
    END IF;
END
$$;
