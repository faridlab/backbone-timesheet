-- DB-level overlap invariant for ranged timesheet entries (Wave 1 P2,
-- pillar-people H-6): two entries of one employee may not overlap when both
-- carry explicit start/end instants. Duration-only rows (NULL bounds) are
-- exempt — Odoo's timesheet grid logs day-grain durations without ranges.
-- btree_gist is required for uuid equality inside a GiST EXCLUDE.

CREATE EXTENSION IF NOT EXISTS btree_gist;

ALTER TABLE timesheet.timesheets
    ADD CONSTRAINT timesheets_no_overlap
    EXCLUDE USING gist (
        company_id WITH =,
        employee_id WITH =,
        tstzrange(time_start, time_end) WITH &&
    )
    WHERE (time_start IS NOT NULL AND time_end IS NOT NULL AND (metadata->>'deleted_at') IS NULL);
