//! The approvals seam (Wave 1 P2, H-6) — the port trait the composing app
//! implements against backbone-approvals once the H-9 decision engine lands.
//!
//! A deliberate mirror of backbone-timeoff's P1 `approvals_port.rs` (proven shape, ADR-0004:
//! shipped libraries keep ZERO normal Cargo edges on each other, so timesheet cannot depend on
//! the approvals crate — the link is data + behavior: `timesheet_approvals.approval_request_id`
//! (a logical FK, no DB constraint across module schemas) + this port, supplied at composition
//! time). Type names carry the `Timesheet` prefix so a host composing BOTH timeoff and timesheet
//! imports no colliding `ApprovalVerdict`/`ApprovalFiling`.
//!
//! P2 scope is the SEAM ONLY: the verbs create and honor the link; the decision engine itself
//! lands with H-9. Until the app wires a real port, [`UnwiredTimesheetApprovals`] is the default
//! and the module behaves exactly as before — periods are approved directly by the manager
//! verbs, and no period carries `approval_request_id` unless someone set it out-of-band (which
//! `approve_period` then fails CLOSED on).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The verdict on a filed approval, as read back through the port. The engine's
/// richer states (escalated, delegated, …) all read as "not yet approved" from here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimesheetVerdict {
    /// Awaiting a decision.
    Pending,
    /// Granted.
    Approved,
    /// Refused (sticky — the engine does not re-ask).
    Rejected,
    /// Withdrawn by the requester.
    Cancelled,
}

/// What timesheet files for approval: WHO submits WHICH period for HOW MANY hours,
/// plus the back-reference so the engine's notifications link back to the period.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimesheetFilingRequest {
    /// The company scope (stamped onto the ApprovalRequest for its own fence).
    pub company_id: Uuid,
    /// The timesheet approval row the filing is about (correlation id).
    pub timesheet_approval_id: Uuid,
    /// The submitting employee # logical FK to employee.Employee.id.
    pub employee_id: Uuid,
    /// The period's year.
    pub year: i32,
    /// The period's month (1–12).
    pub month: i32,
    /// Total logged hours in the period (what the approver sees).
    pub hours: rust_decimal::Decimal,
    /// Applicant note, if any.
    pub note: Option<String>,
    /// When the period was submitted.
    pub submitted_at: DateTime<Utc>,
}

/// Errors from the approvals seam. `Unwired` is the load-bearing variant: it is
/// what the default [`UnwiredTimesheetApprovals`] returns, and what `approve_period`
/// converts into a fail-closed error when a period carries an `approval_request_id`
/// but no port is wired.
#[derive(Debug, thiserror::Error)]
pub enum TimesheetSeamError {
    #[error("the approvals seam is not wired — supply a TimesheetFiling port to use linked approvals")]
    Unwired,
    #[error("approval request {0} not found on the approvals side")]
    UnknownApprovalRequest(Uuid),
    #[error("approvals port transport error: {0}")]
    Transport(String),
}

/// The port (ADR-0004 serialized-port pattern). Implemented by the composing
/// app against backbone-approvals; `timesheet` only ever speaks this trait.
#[async_trait::async_trait]
pub trait TimesheetFiling: Send + Sync {
    /// File a new approval request for a submitted period; returns the created
    /// `approvals.ApprovalRequest.id` to stamp onto `timesheet_approvals.approval_request_id`.
    async fn file(&self, req: &TimesheetFilingRequest) -> Result<Uuid, TimesheetSeamError>;

    /// Read back the verdict for a previously filed approval.
    async fn status(&self, approval_request_id: Uuid) -> Result<TimesheetVerdict, TimesheetSeamError>;
}

/// The default port: nothing is wired. Filing fails loudly (a caller asking for
/// tracked approvals without wiring the engine gets an explicit error, not a
/// silently untracked period); status lookups fail closed for the same reason.
pub struct UnwiredTimesheetApprovals;

#[async_trait::async_trait]
impl TimesheetFiling for UnwiredTimesheetApprovals {
    async fn file(&self, _req: &TimesheetFilingRequest) -> Result<Uuid, TimesheetSeamError> {
        Err(TimesheetSeamError::Unwired)
    }

    async fn status(&self, approval_request_id: Uuid) -> Result<TimesheetVerdict, TimesheetSeamError> {
        Err(TimesheetSeamError::UnknownApprovalRequest(approval_request_id))
    }
}
