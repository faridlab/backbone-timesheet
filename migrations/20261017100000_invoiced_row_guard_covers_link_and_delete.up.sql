-- An invoiced analytic line is frozen until its invoice link is cleared. The guard now runs on
-- every update of a row that carries a link, not only one that keeps it: clearing the link is the
-- reversal path and stays legal on its own, but clearing it while repricing, moving the row to
-- another invoice, or soft-deleting it refuses. Deleting an invoiced row refuses too. The billing
-- writers stamp a link onto unlinked rows and clear a link without touching anything else.
CREATE OR REPLACE FUNCTION timesheet.timesheets_invoiced_row_guard() RETURNS trigger AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'timesheet_invoiced_row_locked: an invoiced analytic line cannot be deleted (clear the invoice link via the reversal path first) — row %', OLD.id;
    END IF;
    IF NEW.invoice_id IS NOT NULL AND NEW.invoice_id IS DISTINCT FROM OLD.invoice_id THEN
        RAISE EXCEPTION 'timesheet_invoiced_row_locked: an invoiced analytic line cannot move to another invoice — row %', OLD.id;
    END IF;
    IF (OLD.metadata->>'deleted_at') IS NULL AND (NEW.metadata->>'deleted_at') IS NOT NULL THEN
        RAISE EXCEPTION 'timesheet_invoiced_row_locked: an invoiced analytic line cannot be deleted (clear the invoice link via the reversal path first) — row %', OLD.id;
    END IF;
    IF OLD.time_start     IS DISTINCT FROM NEW.time_start
    OR OLD.time_end       IS DISTINCT FROM NEW.time_end
    OR OLD.unit_amount    IS DISTINCT FROM NEW.unit_amount
    OR OLD.employee_id    IS DISTINCT FROM NEW.employee_id
    OR OLD.project_id     IS DISTINCT FROM NEW.project_id
    OR OLD.task_id        IS DISTINCT FROM NEW.task_id
    OR OLD.date           IS DISTINCT FROM NEW.date
    OR OLD.year           IS DISTINCT FROM NEW.year
    OR OLD.month          IS DISTINCT FROM NEW.month
    OR OLD.activity_type_id IS DISTINCT FROM NEW.activity_type_id
    OR OLD.is_billable    IS DISTINCT FROM NEW.is_billable
    OR OLD.billing_rate   IS DISTINCT FROM NEW.billing_rate
    OR OLD.costing_rate   IS DISTINCT FROM NEW.costing_rate
    OR OLD.billable_amount IS DISTINCT FROM NEW.billable_amount
    OR OLD.costing_amount IS DISTINCT FROM NEW.costing_amount
    THEN
        RAISE EXCEPTION 'timesheet_invoiced_row_locked: an invoiced analytic line cannot change its pricing or anchoring columns (clear the invoice link via the reversal path first) — row %', OLD.id;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS timesheets_invoiced_row_guard ON timesheet.timesheets;
CREATE TRIGGER timesheets_invoiced_row_guard
    BEFORE UPDATE ON timesheet.timesheets
    FOR EACH ROW
    WHEN (OLD.invoice_id IS NOT NULL)
    EXECUTE FUNCTION timesheet.timesheets_invoiced_row_guard();

DROP TRIGGER IF EXISTS timesheets_invoiced_row_delete_guard ON timesheet.timesheets;
CREATE TRIGGER timesheets_invoiced_row_delete_guard
    BEFORE DELETE ON timesheet.timesheets
    FOR EACH ROW
    WHEN (OLD.invoice_id IS NOT NULL)
    EXECUTE FUNCTION timesheet.timesheets_invoiced_row_guard();
