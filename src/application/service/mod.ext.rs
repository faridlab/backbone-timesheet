// This directory's extension (hand-written; ADR-0031). The generated mod.rs (lib.rs for
// src/) pulls it in with include!, so its lines resolve against this directory.

pub mod overtime_ceiling_port;
// The approvals seam (Wave 1 P2, H-6 — mirrors timeoff P1's port; Timesheet-prefixed types so a
// host composing both modules imports no colliding names). Default Unwired keeps the module
// behaving exactly as before until the app wires a real port against backbone-approvals.
pub mod approvals_port;
// The rate-source seam: billing/costing rate resolution for the analytic line's
// plain-stored snapshots. Default Unwired resolves nothing (rates stay NULL, amounts 0)
// until the app wires a real port over project activity types + employee hourly cost.
pub mod rate_source_port;
// The validated write path (entry CRUD under the period lock, submit window, period transitions,
// plain-stored amounts with reprice-on-qualifying-write, invoiced-row write guard, leave
// regeneration).
pub mod timesheet_write_service;
pub use approvals_port::{
    TimesheetFiling, TimesheetFilingReceipt, TimesheetFilingRequest, TimesheetSeamError,
    TimesheetVerdict, UnwiredTimesheetApprovals,
};
pub use rate_source_port::{
    RateLookup, RateSet, RateSourceError, TimesheetRateSource, UnwiredRateSource,
};
pub use timesheet_write_service::{
    last_day_of_month, TimesheetEntryDto, TimesheetError, TimesheetWriteService,
};
