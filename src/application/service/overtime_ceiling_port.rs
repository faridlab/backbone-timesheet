//! The overtime pre-authorisation ceiling: how much overtime a person is
//! authorised to claim on a given day.
//!
//! The pre-authorisation RECORD lives in the attendance module
//! (overtime_requests, resource 'overtime_request' on the approvals spine);
//! the WRITE paths that must respect it live here. ADR-0004: no crate edge —
//! the composing service injects an adapter over its own attendance reads;
//! the default here is unwired, and an unwired ceiling means NO ceiling
//! (standalone deployments and the module's own tests behave exactly as
//! before the gate existed — it is an enforcement the composition opts
//! into, not a standing trap).

use async_trait::async_trait;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use uuid::Uuid;

#[async_trait]
pub trait OvertimeCeiling: Send + Sync {
    /// The total AUTHORISED overtime hours for (employee, date): the sum of
    /// approved pre-authorisations' planned hours. `None` = no ceiling is
    /// knowable (unwired) — the write paths allow.
    async fn authorised_hours(
        &self,
        employee_id: Uuid,
        date: NaiveDate,
    ) -> Result<Option<Decimal>, String>;
}

/// The unwired default: no ceiling.
pub struct NoCeiling;

#[async_trait]
impl OvertimeCeiling for NoCeiling {
    async fn authorised_hours(
        &self,
        _employee_id: Uuid,
        _date: NaiveDate,
    ) -> Result<Option<Decimal>, String> {
        Ok(None)
    }
}
