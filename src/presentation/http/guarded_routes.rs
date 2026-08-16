//! Guarded route composition — the RECOMMENDED way to mount the timesheet module.
//!
//! Hand-authored (user-owned; see `metaphor.codegen.yaml`). Closes the CRUD-bypass: the
//! generated 12-endpoint CRUD surface writes rows with no domain validation. Here every
//! mutation goes through [`TimesheetWriteService`], which owns the invariants: entry overlap
//! (EXCLUDE-constraint mapped 409), the period lock (entries frozen while pending/approved),
//! the submit validation window (month complete), the one-cycle-per-period rule, and the
//! approvals seam (TR2 fail-closed on linked periods).
//!
//! The tenant comes from the [`CompanyContext`] the `company_auth` middleware inserts — never
//! from the body.

use std::str::FromStr;
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, post, put},
    Json, Router,
};
use backbone_auth::company::CompanyContext;
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::application::service::timesheet_write_service::{
    TimesheetEntryDto, TimesheetError, TimesheetWriteService,
};
use crate::infrastructure::persistence::NewEntry;
use crate::TimesheetModule;

use super::{create_timesheet_approval_read_routes, create_timesheet_read_routes};

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: &'static str,
    message: String,
}

#[derive(Debug, Serialize)]
struct IdResponse {
    id: Uuid,
}

fn err_response(e: TimesheetError) -> axum::response::Response {
    let status = StatusCode::from_u16(e.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(ErrorBody { error: e.code(), message: e.to_string() })).into_response()
}

fn entry_response(dto: TimesheetEntryDto) -> axum::response::Response {
    (StatusCode::OK, Json(dto)).into_response()
}

// ── request bodies ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EntryBody {
    employee_id: Uuid,
    #[serde(default)]
    project_id: Option<Uuid>,
    #[serde(default)]
    task_id: Option<Uuid>,
    date: NaiveDate,
    /// Both bounds ⇒ a ranged entry (overlap-checked); both absent ⇒ duration-only draft.
    #[serde(default)]
    time_start: Option<DateTime<Utc>>,
    #[serde(default)]
    time_end: Option<DateTime<Utc>>,
    #[serde(default)]
    entry_type: Option<String>, // "work" (default) | "overtime"
    #[serde(default)]
    remark: Option<String>,
}

impl EntryBody {
    fn validate(&self) -> Result<NewEntry, TimesheetError> {
        let entry_type = match self.entry_type.as_deref() {
            None => "work",
            Some(s) => match crate::domain::entity::TimesheetType::from_str(s) {
                Ok(crate::domain::entity::TimesheetType::Overtime) => "overtime",
                Ok(_) => "work",
                Err(_) => return Err(TimesheetError::BadEntryType),
            },
        };
        Ok(NewEntry {
            employee_id: self.employee_id,
            project_id: self.project_id,
            task_id: self.task_id,
            date: self.date,
            remark: self.remark.clone(),
            time_start: self.time_start,
            time_end: self.time_end,
            entry_type,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubmitPeriodBody {
    employee_id: Uuid,
    year: i32,
    month: i32,
    #[serde(default)]
    remark: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApprovePeriodBody {
    employee_id: Uuid,
    year: i32,
    month: i32,
    #[serde(default)]
    approver_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RejectPeriodBody {
    employee_id: Uuid,
    year: i32,
    month: i32,
    #[serde(default)]
    remark: Option<String>,
}

// ── handlers ───────────────────────────────────────────────────────────────────

async fn create_entry(
    State(svc): State<Arc<TimesheetWriteService>>,
    tenant: CompanyContext,
    Json(b): Json<EntryBody>,
) -> axum::response::Response {
    let entry = match b.validate() {
        Ok(e) => e,
        Err(e) => return err_response(e),
    };
    match svc.create_entry(tenant.company_id, entry).await {
        Ok(dto) => (StatusCode::CREATED, Json(dto)).into_response(),
        Err(e) => err_response(e),
    }
}

async fn update_entry(
    State(svc): State<Arc<TimesheetWriteService>>,
    tenant: CompanyContext,
    Path(entry_id): Path<Uuid>,
    Json(b): Json<EntryBody>,
) -> axum::response::Response {
    let entry = match b.validate() {
        Ok(e) => e,
        Err(e) => return err_response(e),
    };
    match svc.update_entry(tenant.company_id, entry_id, entry).await {
        Ok(dto) => entry_response(dto),
        Err(e) => err_response(e),
    }
}

async fn delete_entry(
    State(svc): State<Arc<TimesheetWriteService>>,
    tenant: CompanyContext,
    Path(entry_id): Path<Uuid>,
) -> axum::response::Response {
    match svc.delete_entry(tenant.company_id, entry_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err_response(e),
    }
}

async fn submit_period(
    State(svc): State<Arc<TimesheetWriteService>>,
    tenant: CompanyContext,
    Json(b): Json<SubmitPeriodBody>,
) -> axum::response::Response {
    match svc
        .submit_period(tenant.company_id, b.employee_id, b.year, b.month, b.remark, None)
        .await
    {
        Ok(id) => (StatusCode::CREATED, Json(IdResponse { id })).into_response(),
        Err(e) => err_response(e),
    }
}

async fn approve_period(
    State(svc): State<Arc<TimesheetWriteService>>,
    tenant: CompanyContext,
    Json(b): Json<ApprovePeriodBody>,
) -> axum::response::Response {
    match svc
        .approve_period(tenant.company_id, b.employee_id, b.year, b.month, b.approver_id)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err_response(e),
    }
}

async fn reject_period(
    State(svc): State<Arc<TimesheetWriteService>>,
    tenant: CompanyContext,
    Json(b): Json<RejectPeriodBody>,
) -> axum::response::Response {
    match svc
        .reject_period(tenant.company_id, b.employee_id, b.year, b.month, b.remark)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err_response(e),
    }
}

// ── composition ────────────────────────────────────────────────────────────────

/// Build the guarded timesheet router: validated entry + period writes, safe reads, NO generic
/// CRUD mutation. Mount under the host's authenticated (company_auth) tree.
pub fn create_guarded_timesheet_routes(m: &TimesheetModule) -> Router {
    let writes = Router::new()
        .route("/timesheets/entries", post(create_entry))
        .route("/timesheets/entries/:entry_id", put(update_entry))
        .route("/timesheets/entries/:entry_id", delete(delete_entry))
        .route("/timesheets/periods/submit", post(submit_period))
        .route("/timesheets/periods/approve", post(approve_period))
        .route("/timesheets/periods/reject", post(reject_period))
        .with_state(m.timesheet_write_service.clone());

    Router::new()
        .merge(create_timesheet_read_routes(m.timesheet_service.clone()))
        .merge(create_timesheet_approval_read_routes(m.timesheet_approval_service.clone()))
        .merge(writes)
}
