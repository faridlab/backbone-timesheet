-- The lookups a timesheet list makes, which the table had lost.
--
-- The schema declares an (employee_id, date) index, but the table was created
-- with (company_id, employee_id, date); stripping tenancy dropped company_id,
-- and the index went with it. Restored here as declared.
CREATE INDEX IF NOT EXISTS idx_timesheets_employee_id_date
    ON timesheet.timesheets (employee_id, date);

-- Newest first, the order every timesheet list opens in. Partial on live rows,
-- matching the filter the list always applies; id breaks ties in date order.
CREATE INDEX IF NOT EXISTS idx_timesheets_date_live
    ON timesheet.timesheets (date, id)
    WHERE (metadata->>'deleted_at') IS NULL;
