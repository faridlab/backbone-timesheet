-- Back to the guard that ran only while a row kept its invoice link.
DROP TRIGGER IF EXISTS timesheets_invoiced_row_delete_guard ON timesheet.timesheets;
DROP TRIGGER IF EXISTS timesheets_invoiced_row_guard ON timesheet.timesheets;
CREATE TRIGGER timesheets_invoiced_row_guard
    BEFORE UPDATE ON timesheet.timesheets
    FOR EACH ROW
    WHEN (OLD.invoice_id IS NOT NULL AND NEW.invoice_id IS NOT NULL)
    EXECUTE FUNCTION timesheet.timesheets_invoiced_row_guard();
