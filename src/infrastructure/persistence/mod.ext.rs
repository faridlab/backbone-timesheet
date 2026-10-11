// This directory's extension (hand-written; ADR-0031). The generated mod.rs (lib.rs for
// src/) pulls it in with include!, so its lines resolve against this directory.

// Hand-written write SQL for the entry + period-approval flows (user-owned file).
mod timesheet_write_repository;
pub use timesheet_write_repository::{
    EntryRow, EntrySnapshot, EntryWrite, LeaveDayEntry, LeaveRowSync, NewEntry, PeriodRow,
    TimesheetWriteRepository,
};
