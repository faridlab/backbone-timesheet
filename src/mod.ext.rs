// This directory's extension (hand-written; ADR-0031). The generated mod.rs (lib.rs for
// src/) pulls it in with include!, so its lines resolve against this directory.

pub mod request_pool;
// The validated write path (analytic-line entries under the period lock, submit window, period
// transitions, approvals seam — H-6; plain-stored amounts with reprice-on-qualifying-write,
// invoiced-row write guard, leave regeneration) and its guarded HTTP composition, plus
// `org_scope` re-exported for the probe suite (tests/ can only see the public surface,
// mirroring backbone-party).
pub use application::service::{
    last_day_of_month, TimesheetEntryDto, TimesheetError, TimesheetWriteService,
    TimesheetFiling, TimesheetFilingReceipt, TimesheetFilingRequest, TimesheetSeamError,
    TimesheetVerdict, UnwiredTimesheetApprovals,
    RateLookup, RateSet, RateSourceError, TimesheetRateSource, UnwiredRateSource,
};
pub use infrastructure::persistence::{LeaveDayEntry, LeaveRowSync};
pub use presentation::http::create_guarded_timesheet_routes;
pub use backbone_orm::org_scope;
