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
    /// Legacy company leg for the host-owned approvals adapter (ADR-0029): the module
    /// itself is tenant-agnostic and carries no tenant column, but the composing service's
    /// approvals seam still stamps a company onto its ApprovalRequest. The module relays
    /// the ambient org scope's `legacy_company_id` here and fails closed when none is
    /// bound — it never guesses a tenant.
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

/// What a filing came back with: the request the period now points at, the engine's
/// verdict on it at filing time, and whether the engine returned a request that already
/// held this period instead of filing a new one.
///
/// A fresh filing is `Pending` under a policy, or `Approved` straight away for a tenant
/// with no policy. A returned request should only ever be pending or approved, because
/// the engine releases a period once its request is refused; the submit verb still
/// refuses any receipt that carries an earlier decision rather than park the month on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimesheetFilingReceipt {
    /// The `approvals.ApprovalRequest.id` to stamp onto `timesheet_approvals.approval_request_id`.
    pub request_id: Uuid,
    /// The engine's verdict on that request when the filing returned.
    pub verdict: TimesheetVerdict,
    /// true when the engine handed back a request it already held for this period.
    pub already_filed: bool,
}

impl TimesheetFilingReceipt {
    /// Whether the receipt carries a decision taken before this submission: a refused
    /// request, or an approval the engine handed back rather than granted just now.
    /// Marking the month pending on such a request would leave it waiting on an
    /// approver who will never see it.
    pub fn already_decided(&self) -> bool {
        match self.verdict {
            TimesheetVerdict::Pending => false,
            TimesheetVerdict::Approved => self.already_filed,
            TimesheetVerdict::Rejected | TimesheetVerdict::Cancelled => true,
        }
    }
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
    /// File an approval request for a submitted period. The receipt names the request to
    /// stamp onto `timesheet_approvals.approval_request_id` and the engine's verdict on it.
    async fn file(
        &self,
        req: &TimesheetFilingRequest,
    ) -> Result<TimesheetFilingReceipt, TimesheetSeamError>;

    /// Read back the verdict for a previously filed approval.
    async fn status(&self, approval_request_id: Uuid) -> Result<TimesheetVerdict, TimesheetSeamError>;
}

/// The default port: nothing is wired. Filing fails loudly (a caller asking for
/// tracked approvals without wiring the engine gets an explicit error, not a
/// silently untracked period); status lookups fail closed for the same reason.
pub struct UnwiredTimesheetApprovals;

#[async_trait::async_trait]
impl TimesheetFiling for UnwiredTimesheetApprovals {
    async fn file(
        &self,
        _req: &TimesheetFilingRequest,
    ) -> Result<TimesheetFilingReceipt, TimesheetSeamError> {
        Err(TimesheetSeamError::Unwired)
    }

    async fn status(&self, approval_request_id: Uuid) -> Result<TimesheetVerdict, TimesheetSeamError> {
        Err(TimesheetSeamError::UnknownApprovalRequest(approval_request_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt(verdict: TimesheetVerdict, already_filed: bool) -> TimesheetFilingReceipt {
        TimesheetFilingReceipt { request_id: Uuid::nil(), verdict, already_filed }
    }

    #[test]
    fn only_a_fresh_or_pending_request_may_carry_a_month() {
        // A request awaiting a decision, freshly filed or handed back, carries the month.
        assert!(!receipt(TimesheetVerdict::Pending, false).already_decided());
        assert!(!receipt(TimesheetVerdict::Pending, true).already_decided());
        // A tenant with no policy is approved at filing time: that is this submission's verdict.
        assert!(!receipt(TimesheetVerdict::Approved, false).already_decided());
        // An approval handed back belongs to an earlier submission.
        assert!(receipt(TimesheetVerdict::Approved, true).already_decided());
        // A refused request never carries a month, however it came back.
        assert!(receipt(TimesheetVerdict::Rejected, true).already_decided());
        assert!(receipt(TimesheetVerdict::Rejected, false).already_decided());
        assert!(receipt(TimesheetVerdict::Cancelled, true).already_decided());
    }
}
