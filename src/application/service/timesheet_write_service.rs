//! `TimesheetWriteService` — the validated entry + period-approval write path (H-6).
//!
//! Hand-written (user-owned — see `metaphor.codegen.yaml`). Mirrors the proven shapes:
//! backbone-party v0.3.3's write-service (error enum with `code()`/`http_status()`, tx-per-op
//! with `company_scope::bind_company_on`, SQL in the repo) and backbone-timeoff P1's approvals
//! seam (file-first ordering on submit, fail-closed verdict check on approve).
//!
//! The load-bearing invariants:
//! - **Period lock**: a period whose approval row is `pending` or `approved` freezes its
//!   entries — create/update/delete all refuse; `rejected` reopens the period for edits.
//! - **Validation window**: a month can be submitted only once it is COMPLETE (`today` past
//!   the month's last day) — no submitting April while April is still in progress.
//! - **One period cycle per employee-month**: the partial unique index
//!   `(company_id, employee_id, year, month) WHERE deleted_at IS NULL` is the arbiter; the
//!   service turns a rejected row into a re-submittable cycle instead of erroring.
//! - **Overlap**: ranged entries of one employee may not overlap — enforced by the
//!   `timesheets_no_overlap` EXCLUDE constraint, mapped here from 23P01 to a 409.
//! - **TR2 (mirrors timeoff P1)**: a period linked into the approvals engine is approved only
//!   by the engine — `approve_period` fails CLOSED when `approval_request_id` is set and the
//!   port does not return `Approved`.

use std::sync::{Arc, RwLock};

use chrono::{Datelike, DateTime, Months, NaiveDate, Utc};
use rust_decimal::Decimal;
use sqlx::PgPool;
use uuid::Uuid;

use backbone_orm::company_scope;

use crate::infrastructure::persistence::{EntryRow, NewEntry, TimesheetWriteRepository};

use super::approvals_port::{
    TimesheetFiling, TimesheetFilingRequest, TimesheetSeamError, TimesheetVerdict,
    UnwiredTimesheetApprovals,
};

// ─── error surface ────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum TimesheetError {
    #[error("{0} not found")]
    NotFound(&'static str),
    /// The period's approval row is pending or approved — its entries are frozen.
    #[error("period is submitted or approved — entries are frozen")]
    PeriodLocked,
    #[error("period already has a pending or approved submission")]
    PeriodAlreadySubmitted,
    #[error("period is not pending — only a pending period can transition")]
    NotPending,
    /// 23P01 from `timesheets_no_overlap`.
    #[error("this entry overlaps an existing entry")]
    EntryOverlap,
    #[error("time_end must be after time_start")]
    InvalidRange,
    #[error("entryType must be \"work\" or \"overtime\"")]
    BadEntryType,
    #[error("the period has no live entries to submit")]
    EmptyPeriod,
    /// The submit window: the month is not complete yet.
    #[error("the month is not complete yet — submit after month end")]
    WindowNotOpen,
    /// TR2: linked into the engine, not granted by it.
    #[error("approval not granted for the linked approval request")]
    ApprovalNotGranted,
    #[error("approvals seam: {0}")]
    TimesheetSeam(#[from] TimesheetSeamError),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

impl TimesheetError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound(_) => "not_found",
            Self::PeriodLocked => "period_locked",
            Self::PeriodAlreadySubmitted => "period_already_submitted",
            Self::NotPending => "not_pending",
            Self::EntryOverlap => "entry_overlap",
            Self::InvalidRange => "invalid_range",
            Self::BadEntryType => "bad_entry_type",
            Self::EmptyPeriod => "empty_period",
            Self::WindowNotOpen => "window_not_open",
            Self::ApprovalNotGranted => "approval_not_granted",
            Self::TimesheetSeam(_) => "approvals_seam_error",
            Self::Db(_) => "database_error",
        }
    }

    pub fn http_status(&self) -> u16 {
        match self {
            Self::NotFound(_) => 404,
            Self::PeriodLocked | Self::PeriodAlreadySubmitted | Self::NotPending
            | Self::EntryOverlap | Self::ApprovalNotGranted => 409,
            Self::InvalidRange | Self::BadEntryType | Self::EmptyPeriod
            | Self::WindowNotOpen => 422,
            Self::TimesheetSeam(_) | Self::Db(_) => 500,
        }
    }
}

/// The entry as returned over HTTP (camelCase, all the mutable columns back).
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimesheetEntryDto {
    pub id: Uuid,
    pub employee_id: Uuid,
    pub project_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub date: NaiveDate,
    pub remark: Option<String>,
    pub time_start: Option<DateTime<Utc>>,
    pub time_end: Option<DateTime<Utc>>,
    pub entry_type: crate::domain::entity::TimesheetType,
}

impl From<EntryRow> for TimesheetEntryDto {
    fn from(e: EntryRow) -> Self {
        Self {
            id: e.id,
            employee_id: e.employee_id,
            project_id: e.project_id,
            task_id: e.task_id,
            date: e.date,
            remark: e.remark,
            time_start: e.time_start,
            time_end: e.time_end,
            entry_type: e.entry_type,
        }
    }
}

/// The last day of `(year, month)` — the validation window's boundary. Pure.
pub fn last_day_of_month(year: i32, month: i32) -> Option<NaiveDate> {
    let first = NaiveDate::from_ymd_opt(year, month.clamp(1, 12) as u32, 1)?;
    first.checked_add_months(Months::new(1))?.pred_opt()
}

// ─── the service ──────────────────────────────────────────────────────────────

pub struct TimesheetWriteService {
    pool: PgPool,
    repo: TimesheetWriteRepository,
    /// The approvals seam (Wave 1 P2, H-6). Defaults to [`UnwiredTimesheetApprovals`]; the host
    /// swaps in its adapter against backbone-approvals once H-9 lands (ADR-0004: no crate edge).
    /// RwLock (not tokio's) because reads are cloned-and-dropped with no await while held, and
    /// the one write happens at composition time, before serving.
    approvals: RwLock<Arc<dyn TimesheetFiling>>,
}

impl TimesheetWriteService {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            repo: TimesheetWriteRepository,
            approvals: RwLock::new(Arc::new(UnwiredTimesheetApprovals)),
        }
    }

    /// Wire the approvals port (the composing app's adapter). Call once at composition time,
    /// before serving traffic. After this, `submit_period` files every submission and
    /// `approve_period` honors the engine's verdict (TR2).
    pub fn set_approvals(&self, port: Arc<dyn TimesheetFiling>) {
        *self.approvals.write().expect("approvals port lock poisoned") = port;
    }

    fn approvals(&self) -> Arc<dyn TimesheetFiling> {
        self.approvals.read().expect("approvals port lock poisoned").clone()
    }

    // ─── entries (locked while the period is pending/approved) ────────────────

    pub async fn create_entry(
        &self,
        company: Uuid,
        e: NewEntry,
    ) -> Result<TimesheetEntryDto, TimesheetError> {
        validate_entry_bounds(e.time_start, e.time_end)?;
        let now = Utc::now();

        let mut tx = self.pool.begin().await?;
        company_scope::bind_company_on(&mut tx, company).await?;
        self.assert_period_open(&mut tx, company, e.employee_id, e.date).await?;

        let row = self
            .repo
            .insert_entry(&mut tx, company, &e, now)
            .await
            .map_err(map_overlap)?;
        tx.commit().await?;
        Ok(row.into())
    }

    pub async fn update_entry(
        &self,
        company: Uuid,
        entry_id: Uuid,
        e: NewEntry,
    ) -> Result<TimesheetEntryDto, TimesheetError> {
        validate_entry_bounds(e.time_start, e.time_end)?;
        let now = Utc::now();

        let mut tx = self.pool.begin().await?;
        company_scope::bind_company_on(&mut tx, company).await?;
        // The lock guards the row's OWN period as much as the destination — read the source
        // period off the entry itself (delete_entry's rule: row-truth over client-truth), so a
        // caller can't move hours OUT of a frozen period by re-dating them into an open one.
        // The row's employee is authoritative for BOTH checks; the payload's employee_id is
        // not written by the update and must not widen the lock lookup.
        let (row_employee, row_year, row_month) = self
            .repo
            .entry_period(&mut tx, company, entry_id)
            .await?
            .ok_or(TimesheetError::NotFound("timesheet entry"))?;
        self.assert_period_open_ym(&mut tx, company, row_employee, row_year, row_month).await?;
        self.assert_period_open(&mut tx, company, row_employee, e.date).await?;

        let row = self
            .repo
            .update_entry(&mut tx, company, entry_id, &e, now)
            .await
            .map_err(map_overlap)?
            .ok_or(TimesheetError::NotFound("timesheet entry"))?;
        tx.commit().await?;
        Ok(row.into())
    }

    pub async fn delete_entry(&self, company: Uuid, entry_id: Uuid) -> Result<(), TimesheetError> {
        let now = Utc::now();

        let mut tx = self.pool.begin().await?;
        company_scope::bind_company_on(&mut tx, company).await?;
        // The lock is per (employee, year, month) — read it off the entry itself so a caller
        // can't mutate around the lock by omitting the period.
        let (employee_id, year, month) = self
            .repo
            .entry_period(&mut tx, company, entry_id)
            .await?
            .ok_or(TimesheetError::NotFound("timesheet entry"))?;
        self.assert_period_open_ym(&mut tx, company, employee_id, year, month).await?;

        let deleted = self.repo.soft_delete_entry(&mut tx, company, entry_id, now).await?;
        if !deleted {
            tx.rollback().await?;
            return Err(TimesheetError::NotFound("timesheet entry"));
        }
        tx.commit().await?;
        Ok(())
    }

    // ─── period cycle: submit → approve / reject ───────────────────────────────

    /// Submit an employee's month for approval. Gates: the month must be COMPLETE (validation
    /// window), the period must hold at least one live entry, and it must not already be
    /// pending/approved (a rejected period re-opens into a new cycle). Files with the approvals
    /// engine when the seam is wired (file-first, timeoff's ordering: the filing carries the
    /// period id so the insert lands with `approval_request_id` already set; an unwired seam
    /// means this deployment doesn't track approvals — the period simply carries no link).
    pub async fn submit_period(
        &self,
        company: Uuid,
        employee_id: Uuid,
        year: i32,
        month: i32,
        remark: Option<String>,
        now: Option<DateTime<Utc>>,
    ) -> Result<Uuid, TimesheetError> {
        let now = now.unwrap_or_else(Utc::now);

        // Validation window: today must be past the month's last day.
        let last = last_day_of_month(year, month)
            .ok_or(TimesheetError::InvalidRange)?;
        if now.date_naive() <= last {
            return Err(TimesheetError::WindowNotOpen);
        }

        let mut tx = self.pool.begin().await?;
        company_scope::bind_company_on(&mut tx, company).await?;

        let existing = self.repo.period_row(&mut tx, company, employee_id, year, month).await?;
        if let Some(p) = &existing {
            if p.status == "pending" || p.status == "approved" {
                return Err(TimesheetError::PeriodAlreadySubmitted);
            }
        }
        if self.repo.live_entry_count(&mut tx, company, employee_id, year, month).await? == 0 {
            return Err(TimesheetError::EmptyPeriod);
        }
        let hours = self.repo.sum_period_hours(&mut tx, company, employee_id, year, month).await?;
        // Release the snapshot before the port call — the filing is a network hop; holding a
        // tx across it buys nothing (the transition below is guarded by status predicates).
        let revive_id = existing.as_ref().map(|p| p.id);
        let period_id = revive_id.unwrap_or_else(Uuid::new_v4);
        tx.commit().await?;

        let filing = TimesheetFilingRequest {
            company_id: company,
            timesheet_approval_id: period_id,
            employee_id,
            year,
            month,
            hours,
            note: remark.clone(),
            submitted_at: now,
        };
        let approval_request_id = match self.approvals().file(&filing).await {
            Ok(id) => Some(id),
            Err(TimesheetSeamError::Unwired) => None,
            Err(e) => return Err(e.into()),
        };

        let mut tx = self.pool.begin().await?;
        company_scope::bind_company_on(&mut tx, company).await?;
        match revive_id {
            Some(id) => self
                .repo
                .revive_period_pending(&mut tx, id, remark.as_deref(), approval_request_id, now)
                .await?,
            None => self
                .repo
                .insert_period_pending(&mut tx, period_id, company, employee_id, year, month, remark.as_deref(), approval_request_id, now)
                .await?,
        }
        tx.commit().await?;
        Ok(period_id)
    }

    /// Approve a pending period. TR2 (mirrors timeoff P1): when the period carries an
    /// `approval_request_id`, the ONLY way through is the engine returning `Approved` — an
    /// unwired port or an unknown filing fails CLOSED, never bypasses. The billable aggregate is
    /// computed and stamped under the same tx as the transition.
    pub async fn approve_period(
        &self,
        company: Uuid,
        employee_id: Uuid,
        year: i32,
        month: i32,
        approver_id: Option<Uuid>,
    ) -> Result<(), TimesheetError> {
        let now = Utc::now();
        let mut tx = self.pool.begin().await?;
        company_scope::bind_company_on(&mut tx, company).await?;

        let period = self
            .repo
            .period_row(&mut tx, company, employee_id, year, month)
            .await?
            .ok_or(TimesheetError::NotFound("timesheet period"))?;
        if period.status != "pending" {
            return Err(TimesheetError::NotPending);
        }

        // TR2 verdict check happens BEFORE the transition commits; the UPDATE below only moves
        // pending rows anyway, so a verdict flip mid-flight turns into this same error on retry.
        if let Some(approval_request_id) = period.approval_request_id {
            match self.approvals().status(approval_request_id).await {
                Ok(TimesheetVerdict::Approved) => {}
                Ok(_) => return Err(TimesheetError::ApprovalNotGranted),
                // Unwired port + a linked period = out-of-band linkage or a deployment
                // regression — fail CLOSED: never bypass the engine a period was filed into.
                Err(TimesheetSeamError::Unwired)
                | Err(TimesheetSeamError::UnknownApprovalRequest(_)) => {
                    return Err(TimesheetError::ApprovalNotGranted)
                }
                Err(e) => return Err(e.into()),
            }
        }

        let hours = self.repo.sum_period_hours(&mut tx, company, employee_id, year, month).await?;
        let moved = self
            .repo
            .mark_period_approved(&mut tx, period.id, approver_id, hours, now)
            .await?;
        if !moved {
            return Err(TimesheetError::NotPending);
        }
        tx.commit().await?;
        Ok(())
    }

    /// Reject a pending period — the period reopens for edits (`rejected` does not lock).
    pub async fn reject_period(
        &self,
        company: Uuid,
        employee_id: Uuid,
        year: i32,
        month: i32,
        remark: Option<String>,
    ) -> Result<(), TimesheetError> {
        let now = Utc::now();
        let mut tx = self.pool.begin().await?;
        company_scope::bind_company_on(&mut tx, company).await?;

        let period = self
            .repo
            .period_row(&mut tx, company, employee_id, year, month)
            .await?
            .ok_or(TimesheetError::NotFound("timesheet period"))?;
        if period.status != "pending" {
            return Err(TimesheetError::NotPending);
        }

        let moved = self
            .repo
            .mark_period_rejected(&mut tx, period.id, remark.as_deref(), now)
            .await?;
        if !moved {
            return Err(TimesheetError::NotPending);
        }
        tx.commit().await?;
        Ok(())
    }

    // ─── lock helper ────────────────────────────────────────────────────────────

    /// Entries are frozen while the period's approval row is `pending` or `approved`;
    /// `rejected` (or no row yet) leaves the period open for edits.
    async fn assert_period_open(
        &self,
        conn: &mut sqlx::PgConnection,
        company: Uuid,
        employee_id: Uuid,
        date: NaiveDate,
    ) -> Result<(), TimesheetError> {
        self.assert_period_open_ym(conn, company, employee_id, date.year(), date.month() as i32)
            .await
    }

    async fn assert_period_open_ym(
        &self,
        conn: &mut sqlx::PgConnection,
        company: Uuid,
        employee_id: Uuid,
        year: i32,
        month: i32,
    ) -> Result<(), TimesheetError> {
        if let Some(p) = self.repo.period_row(conn, company, employee_id, year, month).await? {
            if p.status == "pending" || p.status == "approved" {
                return Err(TimesheetError::PeriodLocked);
            }
        }
        Ok(())
    }
}

// ─── small helpers ────────────────────────────────────────────────────────────

fn validate_entry_bounds(
    time_start: Option<DateTime<Utc>>,
    time_end: Option<DateTime<Utc>>,
) -> Result<(), TimesheetError> {
    if let (Some(s), Some(e)) = (time_start, time_end) {
        if e <= s {
            return Err(TimesheetError::InvalidRange);
        }
    }
    Ok(())
}

/// Map a DB error carrying the entries EXCLUDE constraint (23P01) to the 409 the API promises.
fn map_overlap(e: sqlx::Error) -> TimesheetError {
    let hit = e
        .as_database_error()
        .map(|d| d.constraint().map(|c| c.contains("no_overlap")).unwrap_or(false))
        .unwrap_or(false);
    if hit {
        TimesheetError::EntryOverlap
    } else {
        TimesheetError::Db(e)
    }
}
