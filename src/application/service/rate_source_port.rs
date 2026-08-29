//! The rate-source seam — the port the composing app implements to resolve
//! billing/costing rates for the analytic line (the hr_timesheet hourly-cost
//! tail).
//!
//! A deliberate mirror of this module's approvals seam (`approvals_port.rs`,
//! ADR-0004: shipped libraries keep ZERO normal Cargo edges on each other, so
//! timesheet cannot read `project.activity_types` or an employee cost store
//! directly — the link is this port, supplied at composition time).
//!
//! Rate ladder the HOST adapter owns (the module only consumes the resolved
//! `RateSet`):
//!
//! 1. employee hourly cost (from the employee master, when that store exists);
//! 2. `project.activity_types.costing_rate` fallback;
//! 3. NULL — `costing_rate` visibly NULL, `costing_amount` 0.
//!
//! Billing rate comes only from `project.activity_types.billing_rate`.
//! A later (project, employee) rate map, when ported, inserts ABOVE employee
//! hourly cost in that ladder — reserved headroom, nothing here hard-codes the
//! order.
//!
//! Called ONLY from the write path on qualifying writes (a write touching
//! [time_start, time_end, unit_amount, employee_id, activity_type_id]
//! re-resolves rates and synchronously rewrites the rate/amount snapshots in
//! the same transaction; every other write keeps the stored snapshot, and no
//! read ever reprices). Until the app wires a real port,
//! [`UnwiredRateSource`] is the default: it resolves nothing, so rates stay
//! NULL and amounts stay 0 — the module builds and behaves unwired.

use rust_decimal::Decimal;
use uuid::Uuid;

/// What the write path asks: which company, whose time, classified how.
/// `employee_id` is the row's own employee (the resolution — no fallback
/// ladder); `activity_type_id` is the row's classification when set.
#[derive(Debug, Clone)]
pub struct RateLookup {
    pub company_id: Uuid,
    pub employee_id: Option<Uuid>,
    pub activity_type_id: Option<Uuid>,
}

/// The resolved rate pair the write path snapshots onto the row.
/// `None` = rate unknown: the stored rate stays NULL and the matching amount
/// stays 0 — a visible absence, never an invented figure.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RateSet {
    pub billing_rate: Option<Decimal>,
    pub costing_rate: Option<Decimal>,
}

/// Errors from the rate seam. Deliberately plain (code + message): the write
/// path wraps it into its own typed error surface.
#[derive(Debug, thiserror::Error)]
#[error("rate source {code}: {message}")]
pub struct RateSourceError {
    pub code: String,
    pub message: String,
}

/// The port (ADR-0004 serialized-port pattern). Implemented by the composing
/// app over `project.activity_types` (raw cross-schema read, company-scoped)
/// plus the employee hourly-cost store when it exists; `timesheet` only ever
/// speaks this trait.
#[async_trait::async_trait]
pub trait TimesheetRateSource: Send + Sync {
    /// Resolve the billing/costing rate pair for a qualifying write.
    async fn resolve_rates(&self, req: &RateLookup) -> Result<RateSet, RateSourceError>;
}

/// The default port: nothing is wired. Resolves an empty [`RateSet`] (not an
/// error) — until the host composes a rate source, rates are visibly NULL and
/// amounts 0, exactly the posture of rows written before any rate source
/// existed.
pub struct UnwiredRateSource;

#[async_trait::async_trait]
impl TimesheetRateSource for UnwiredRateSource {
    async fn resolve_rates(&self, _req: &RateLookup) -> Result<RateSet, RateSourceError> {
        Ok(RateSet::default())
    }
}
