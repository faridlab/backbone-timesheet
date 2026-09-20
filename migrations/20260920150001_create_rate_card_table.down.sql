-- Down: drop timesheet.rate_cards table
DROP TABLE IF EXISTS timesheet.rate_cards CASCADE;
DROP FUNCTION IF EXISTS timesheet.rate_cards_audit_timestamp() CASCADE;
