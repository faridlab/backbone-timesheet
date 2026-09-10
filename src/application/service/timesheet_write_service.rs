//! `TimesheetWriteService` — the validated analytic-line + period-approval write path.
//!
//! Hand-written (user-owned — see `metaphor.codegen.yaml`). Mirrors the proven shapes:
//! backbone-party v0.3.3's write-service (error enum with `code()`/`http_status()`, tx-per-op
//! with SQL in the repo) and backbone-timeoff P1's approvals seam (file-first ordering on
//! submit, fail-closed verdict check on approve).
//!
//! Tenancy (ADR-0029): the module is tenant-agnostic — no verb takes a tenant id and no
//! statement names a tenant column. Each verb's transaction relays the AMBIENT org scope
//! (`org_scope::current_org_scope` → `org_scope::bind_org_scope_on`); isolation is owned by
//! the composing service's tenancy decorator, and an unscoped caller simply runs unfenced.
//! The two company-keyed host seams — the rate-source lookup and the approvals filing —
//! receive the scope's `legacy_company_id` leg and fail closed
//! ([`TimesheetError::NoCompanyScope`]) when the composing service resolved none.
//!
//! The load-bearing invariants:
//! - **Period lock**: a period whose approval row is `pending` or `approved` freezes its
//!   entries — create/update/delete all refuse; `rejected` reopens the period for edits.
//! - **Validation window**: a month can be submitted only once it is COMPLETE (`today` past
//!   the month's last day) — no submitting April while April is still in progress.
//! - **One period cycle per employee-month**: the partial unique index
//!   `(employee_id, year, month) WHERE (metadata->>'deleted_at') IS NULL` is the arbiter; the
//!   service turns a rejected row into a re-submittable cycle instead of erroring.
//! - **Overlap**: ranged entries of one employee may not overlap — enforced by the
//!   `timesheets_no_overlap` EXCLUDE constraint, mapped here from 23P01 to a 409.
//! - **TR2 (mirrors timeoff P1)**: a period linked into the approvals engine is approved only
//!   by the engine — `approve_period` fails CLOSED when `approval_request_id` is set and the
//!   port does not return `Approved`.
//! - **Plain-stored amounts**: `unit_amount`, the rate snapshots, and the amounts are STORED
//!   columns — no read ever reprices. A write touching any of
//!   [time_start, time_end, unit_amount, employee_id, activity_type_id] re-resolves rates
//!   through the [`TimesheetRateSource`](super::rate_source_port::TimesheetRateSource) port and
//!   synchronously rewrites the snapshots in the SAME transaction; any other write (remark,
//!   task/project anchoring, date) keeps the stored snapshot. An `is_billable` flip recomputes
//!   `billable_amount` from the STORED rate snapshot without re-resolving rates.
//! - **No state on the row**: the row carries no approval of its own — the per-employee/month
//!   approval cycle (`timesheet_approvals`) is the billability gate. `invoice_id` is a one-way
//!   link stamped by the billing exit, cleared on reversal.
//! - **Invoiced-row write guard**: once a row carries `invoice_id`, update and delete refuse
//!   with a typed error; a DB trigger backstops raw writers. Clearing the link (the reversal
//!   path) stays legal.
//! - **Leave regeneration**: delete-and-regenerate off the leave request's origin key —
//!   authoritative over its OWN rows even in a locked/approved period (the leave lifecycle
//!   rules those rows), refusing loudly when any carries an invoice link; ordinary rows stay
//!   under the period lock. The `(source_timeoff_request_id, date)` partial unique
//!   is the no-duplicates DB backstop.

use std::sync::{Arc, RwLock};

use chrono::{DateTime, Datelike, Months, NaiveDate, Utc};
use rust_decimal::Decimal;
use sqlx::PgPool;
use uuid::Uuid;

use backbone_orm::org_scope;

use crate::infrastructure::persistence::{
    EntryRow, EntrySnapshot, EntryWrite, LeaveRowSync, NewEntry, TimesheetWriteRepository,
};

use super::approvals_port::{
    TimesheetFiling, TimesheetFilingRequest, TimesheetSeamError, TimesheetVerdict,
    UnwiredTimesheetApprovals,
};
use super::rate_source_port::{RateLookup, RateSet, TimesheetRateSource, UnwiredRateSource};

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
    #[error("explicit hours cannot be negative")]
    NegativeHours,
    #[error("the period has no live entries to submit")]
    EmptyPeriod,
    /// The submit window: the month is not complete yet.
    #[error("the month is not complete yet — submit after month end")]
    WindowNotOpen,
    /// TR2: linked into the engine, not granted by it.
    #[error("approval not granted for the linked approval request")]
    ApprovalNotGranted,
    /// The row carries an invoice link — its pricing/anchoring columns are frozen.
    #[error("the entry is linked to an invoice — clear the link via the reversal path first")]
    InvoicedRowLocked,
    /// Regeneration would rewrite billed absence — a loud operator case, never silent.
    #[error("the leave request has billed rows — reverse the invoice before regenerating")]
    LeaveRowBilled,
    /// A company-keyed host seam needed a tenant and the ambient org scope resolved none —
    /// the composing service did not bind one (or runs unfenced while such an adapter is
    /// wired). Fail closed rather than guess.
    #[error("no org scope bound: the composing service must resolve one for this request")]
    NoCompanyScope,
    #[error("approvals seam: {0}")]
    TimesheetSeam(#[from] TimesheetSeamError),
    #[error("rate source: {0}")]
    RateSource(#[from] super::rate_source_port::RateSourceError),
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
            Self::NegativeHours => "negative_hours",
            Self::EmptyPeriod => "empty_period",
            Self::WindowNotOpen => "window_not_open",
            Self::ApprovalNotGranted => "approval_not_granted",
            Self::InvoicedRowLocked => "invoiced_row_locked",
            Self::LeaveRowBilled => "leave_row_billed",
            Self::NoCompanyScope => "no_org_scope",
            Self::TimesheetSeam(_) => "approvals_seam_error",
            Self::RateSource(_) => "rate_source_error",
            Self::Db(_) => "database_error",
        }
    }

    pub fn http_status(&self) -> u16 {
        match self {
            Self::NotFound(_) => 404,
            Self::PeriodLocked | Self::PeriodAlreadySubmitted | Self::NotPending
            | Self::EntryOverlap | Self::ApprovalNotGranted | Self::InvoicedRowLocked
            | Self::LeaveRowBilled => 409,
            Self::InvalidRange | Self::BadEntryType | Self::NegativeHours | Self::EmptyPeriod
            | Self::WindowNotOpen => 422,
            Self::NoCompanyScope | Self::TimesheetSeam(_) | Self::RateSource(_)
            | Self::Db(_) => 500,
        }
    }
}

/// The entry as returned over HTTP (camelCase, every stored column back — the
/// plain-stored snapshots included, read as stored, never recomputed).
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
    pub unit_amount: Decimal,
    pub currency: String,
    pub activity_type_id: Option<Uuid>,
    pub billing_rate: Option<Decimal>,
    pub costing_rate: Option<Decimal>,
    pub is_billable: bool,
    pub billable_amount: Decimal,
    pub costing_amount: Decimal,
    pub invoice_id: Option<Uuid>,
    pub source_timeoff_request_id: Option<Uuid>,
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
            unit_amount: e.unit_amount,
            currency: e.currency,
            activity_type_id: e.activity_type_id,
            billing_rate: e.billing_rate,
            costing_rate: e.costing_rate,
            is_billable: e.is_billable,
            billable_amount: e.billable_amount,
            costing_amount: e.costing_amount,
            invoice_id: e.invoice_id,
            source_timeoff_request_id: e.source_timeoff_request_id,
        }
    }
}

/// The last day of `(year, month)` — the validation window's boundary. Pure.
pub fn last_day_of_month(year: i32, month: i32) -> Option<NaiveDate> {
    let first = NaiveDate::from_ymd_opt(year, month.clamp(1, 12) as u32, 1)?;
    first.checked_add_months(Months::new(1))?.pred_opt()
}

/// Hours from a time window, plain-stored to 2dp (round half away from zero — the
/// ecosystem's money rounding). Pure. `None` when either bound is absent.
fn hours_from_windows(
    time_start: Option<DateTime<Utc>>,
    time_end: Option<DateTime<Utc>>,
) -> Option<Decimal> {
    let (s, e) = (time_start?, time_end?);
    let secs = (e - s).num_seconds();
    Some(Decimal::from(secs) / Decimal::from(3600)).map(|h| h.round_dp(2))
}

/// The plain-stored amounts derived from hours + the resolved/kept rate snapshots.
/// A NULL rate means "rate unknown": the matching amount is 0 — a visible absence
/// in the rate column, never an invented figure.
fn amounts_from(
    unit_amount: Decimal,
    is_billable: bool,
    billing_rate: Option<Decimal>,
    costing_rate: Option<Decimal>,
) -> (Decimal, Decimal) {
    let billable = if is_billable {
        billing_rate.map(|r| (unit_amount * r).round_dp(2)).unwrap_or(Decimal::ZERO)
    } else {
        Decimal::ZERO
    };
    let costing = costing_rate.map(|r| (unit_amount * r).round_dp(2)).unwrap_or(Decimal::ZERO);
    (billable, costing)
}

/// Fail-closed legacy company twin: the composing service's rate-source and approvals
/// adapters still key their lookups on a company id (those seams are host-owned and keep
/// their company axis until the owning modules strip — ADR-0029). Resolved from the ambient
/// org scope's `legacy_company_id` leg; absent it, the caller fails closed rather than
/// guessing a tenant.
fn legacy_company_id() -> Result<Uuid, TimesheetError> {
    org_scope::current_org_scope()
        .and_then(|s| s.legacy_company_id())
        .ok_or(TimesheetError::NoCompanyScope)
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
    /// The rate-source seam. Defaults to [`UnwiredRateSource`] (resolves nothing — rates stay
    /// NULL, amounts 0) until the host composes an adapter over project activity types and the
    /// employee hourly-cost store.
    rates: RwLock<Arc<dyn TimesheetRateSource>>,
}

impl TimesheetWriteService {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            repo: TimesheetWriteRepository,
            approvals: RwLock::new(Arc::new(UnwiredTimesheetApprovals)),
            rates: RwLock::new(Arc::new(UnwiredRateSource)),
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

    /// Wire the rate-source port (the composing app's adapter over
    /// `project.activity_types` + employee hourly cost). Call once at composition time.
    /// After this, qualifying writes snapshot resolved rates onto the row.
    pub fn set_rate_source(&self, port: Arc<dyn TimesheetRateSource>) {
        *self.rates.write().expect("rate source lock poisoned") = port;
    }

    fn rates(&self) -> Arc<dyn TimesheetRateSource> {
        self.rates.read().expect("rate source lock poisoned").clone()
    }

    /// Begin the verb's transaction with the ambient org scope relayed onto it (ADR-0029).
    /// Relay-only: the module never invents a scope — a caller with none bound simply runs
    /// unfenced (isolation is the composing service's decorator, not this transaction's job).
    async fn scoped_tx(
        &self,
    ) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, TimesheetError> {
        let mut tx = self.pool.begin().await?;
        if let Some(scope) = org_scope::current_org_scope() {
            org_scope::bind_org_scope_on(&mut tx, &scope).await?;
        }
        Ok(tx)
    }

    /// Resolve rates through the port for a qualifying write. The lookup's employee is the
    /// ROW's employee (the resolution — there is no fallback ladder to port); the lookup's
    /// company is the ambient scope's legacy leg (fail-closed — see [`legacy_company_id`]).
    async fn resolve_rates(
        &self,
        employee_id: Uuid,
        activity_type_id: Option<Uuid>,
    ) -> Result<RateSet, TimesheetError> {
        self.rates()
            .resolve_rates(&RateLookup {
                company_id: legacy_company_id()?,
                employee_id: Some(employee_id),
                activity_type_id,
            })
            .await
            .map_err(TimesheetError::RateSource)
    }

    // ─── entries (locked while the period is pending/approved) ────────────────

    pub async fn create_entry(&self, e: NewEntry) -> Result<TimesheetEntryDto, TimesheetError> {
        validate_entry_bounds(e.time_start, e.time_end)?;
        if let Some(h) = e.hours {
            if h < Decimal::ZERO {
                return Err(TimesheetError::NegativeHours);
            }
        }
        // Plain-stored hours: the windows win when both bounds are present; else the explicit
        // hours input; else 0 (honest absence — hours are not invented).
        let unit_amount = hours_from_windows(e.time_start, e.time_end)
            .or(e.hours.map(|h| h.round_dp(2)))
            .unwrap_or(Decimal::ZERO);
        let is_billable = e.is_billable.unwrap_or(true);

        let mut tx = self.scoped_tx().await?;
        self.assert_period_open(&mut tx, e.employee_id, e.date).await?;

        // A create is a qualifying write by definition: resolve rates through the port and
        // stamp the snapshots in the same transaction (employee resolution IS the row's
        // employee_id — no ladder).
        let rates = self.resolve_rates(e.employee_id, e.activity_type_id).await?;
        let (billable_amount, costing_amount) =
            amounts_from(unit_amount, is_billable, rates.billing_rate, rates.costing_rate);

        let w = EntryWrite {
            project_id: e.project_id,
            task_id: e.task_id,
            date: e.date,
            remark: e.remark.clone(),
            time_start: e.time_start,
            time_end: e.time_end,
            entry_type: e.entry_type,
            unit_amount,
            activity_type_id: e.activity_type_id,
            billing_rate: rates.billing_rate,
            costing_rate: rates.costing_rate,
            is_billable,
            billable_amount,
            costing_amount,
        };

        let row = self
            .repo
            .insert_entry(&mut tx, e.employee_id, &w, Utc::now())
            .await
            .map_err(map_entry_write_error)?;
        tx.commit().await?;
        Ok(row.into())
    }

    pub async fn update_entry(
        &self,
        entry_id: Uuid,
        e: NewEntry,
    ) -> Result<TimesheetEntryDto, TimesheetError> {
        validate_entry_bounds(e.time_start, e.time_end)?;
        if let Some(h) = e.hours {
            if h < Decimal::ZERO {
                return Err(TimesheetError::NegativeHours);
            }
        }
        let now = Utc::now();

        let mut tx = self.scoped_tx().await?;
        // The lock guards the row's OWN period as much as the destination — read the source
        // period off the entry itself (delete_entry's rule: row-truth over client-truth), so a
        // caller can't move hours OUT of a frozen period by re-dating them into an open one.
        // The row's employee is authoritative for BOTH checks; the payload's employee_id is
        // not written by the update and must not widen the lock lookup.
        let snap: EntrySnapshot = self
            .repo
            .entry_snapshot(&mut tx, entry_id)
            .await?
            .ok_or(TimesheetError::NotFound("timesheet entry"))?;
        // Invoiced rows are frozen (a link, not a state — but it writes like one): the
        // reversal path clears the link first; a DB trigger backstops raw writers.
        if snap.invoice_id.is_some() {
            return Err(TimesheetError::InvoicedRowLocked);
        }
        self.assert_period_open_ym(&mut tx, snap.employee_id, snap.year, snap.month)
            .await?;
        self.assert_period_open(&mut tx, snap.employee_id, e.date).await?;

        // Plain-stored hours: windows win; else explicit input; else the row KEEPS its hours
        // (a remark-only edit must not zero a duration-only row).
        let unit_amount = hours_from_windows(e.time_start, e.time_end)
            .or(e.hours.map(|h| h.round_dp(2)))
            .unwrap_or(snap.unit_amount);

        // Reprice only on a qualifying write: a change to the hours figure, the windows, or
        // the activity classification (employee_id is not writable on update — when a future
        // write path can move it, it joins this set). Everything else keeps the snapshot.
        let qualifying = snap.time_start != e.time_start
            || snap.time_end != e.time_end
            || unit_amount != snap.unit_amount
            || snap.activity_type_id != e.activity_type_id;
        let rates = if qualifying {
            self.resolve_rates(snap.employee_id, e.activity_type_id).await?
        } else {
            RateSet { billing_rate: snap.billing_rate, costing_rate: snap.costing_rate }
        };
        // An is_billable flip recomputes from the STORED (or just re-resolved) snapshot —
        // never re-resolves rates on its own.
        let is_billable = e.is_billable.unwrap_or(snap.is_billable);
        let (billable_amount, costing_amount) =
            amounts_from(unit_amount, is_billable, rates.billing_rate, rates.costing_rate);

        let w = EntryWrite {
            project_id: e.project_id,
            task_id: e.task_id,
            date: e.date,
            remark: e.remark.clone(),
            time_start: e.time_start,
            time_end: e.time_end,
            entry_type: e.entry_type,
            unit_amount,
            activity_type_id: e.activity_type_id,
            billing_rate: rates.billing_rate,
            costing_rate: rates.costing_rate,
            is_billable,
            billable_amount,
            costing_amount,
        };

        let row = self
            .repo
            .update_entry(&mut tx, entry_id, &w, now)
            .await
            .map_err(map_entry_write_error)?
            .ok_or(TimesheetError::NotFound("timesheet entry"))?;
        tx.commit().await?;
        Ok(row.into())
    }

    pub async fn delete_entry(&self, entry_id: Uuid) -> Result<(), TimesheetError> {
        let now = Utc::now();

        let mut tx = self.scoped_tx().await?;
        // The lock is per (employee, year, month) — read it off the entry itself so a caller
        // can't mutate around the lock by omitting the period. An invoiced row refuses:
        // billed hours are corrected by reversal, never deleted out from under the invoice.
        let snap = self
            .repo
            .entry_snapshot(&mut tx, entry_id)
            .await?
            .ok_or(TimesheetError::NotFound("timesheet entry"))?;
        if snap.invoice_id.is_some() {
            return Err(TimesheetError::InvoicedRowLocked);
        }
        self.assert_period_open_ym(&mut tx, snap.employee_id, snap.year, snap.month)
            .await?;

        let deleted = self.repo.soft_delete_entry(&mut tx, entry_id, now).await?;
        if !deleted {
            tx.rollback().await?;
            return Err(TimesheetError::NotFound("timesheet entry"));
        }
        tx.commit().await?;
        Ok(())
    }

    // ─── leave regeneration (authoritative over its OWN rows) ─────────────────

    /// Delete-and-regenerate the timesheet rows mirroring one leave request's settled window
    /// (approve/refuse/cancel/void all converge here — the caller maps the settlement to the
    /// day-by-day hours it wants reflected; refused/cancelled/voided requests regenerate to
    /// zero rows, i.e. just the delete). Never updates in place: rows are soft-deleted by
    /// origin key and re-inserted per day, inside ONE transaction.
    ///
    /// Authoritative over its OWN rows even in a locked/approved period — the leave lifecycle
    /// rules those rows; ordinary rows in the same period stay frozen under the period lock.
    /// Refuses loudly when any of the request's rows already carried an invoice link (billed
    /// absence is corrected by reversing the invoice first). The partial unique
    /// `(source_timeoff_request_id, date)` among live rows is the DB backstop
    /// making duplicates impossible. Leave rows carry `entry_type='timeoff'`, NULL rates and
    /// zero amounts (no rate source applies to absence).
    ///
    /// Returns the number of rows inserted.
    pub async fn regenerate_leave_rows(&self, sync: &LeaveRowSync) -> Result<u32, TimesheetError> {
        let now = Utc::now();
        let mut tx = self.scoped_tx().await?;

        let billed = self
            .repo
            .count_billed_leave_rows(&mut tx, sync.timeoff_request_id)
            .await?;
        if billed > 0 {
            return Err(TimesheetError::LeaveRowBilled);
        }

        self.repo
            .soft_delete_leave_rows(&mut tx, sync.timeoff_request_id, now)
            .await?;

        let mut inserted = 0u32;
        for day in &sync.entries {
            if day.hours < Decimal::ZERO {
                return Err(TimesheetError::NegativeHours);
            }
            self.repo.insert_leave_row(&mut tx, sync, day, now).await?;
            inserted += 1;
        }
        tx.commit().await?;
        Ok(inserted)
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

        let mut tx = self.scoped_tx().await?;

        let existing = self.repo.period_row(&mut tx, employee_id, year, month).await?;
        if let Some(p) = &existing {
            if p.status == "pending" || p.status == "approved" {
                return Err(TimesheetError::PeriodAlreadySubmitted);
            }
        }
        if self.repo.live_entry_count(&mut tx, employee_id, year, month).await? == 0 {
            return Err(TimesheetError::EmptyPeriod);
        }
        let hours = self.repo.sum_period_hours(&mut tx, employee_id, year, month).await?;
        // Release the snapshot before the port call — the filing is a network hop; holding a
        // tx across it buys nothing (the transition below is guarded by status predicates).
        let revive_id = existing.as_ref().map(|p| p.id);
        let period_id = revive_id.unwrap_or_else(Uuid::new_v4);
        tx.commit().await?;

        let filing = TimesheetFilingRequest {
            company_id: legacy_company_id()?,
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

        let mut tx = self.scoped_tx().await?;
        match revive_id {
            Some(id) => self
                .repo
                .revive_period_pending(&mut tx, id, remark.as_deref(), approval_request_id, now)
                .await?,
            None => self
                .repo
                .insert_period_pending(&mut tx, period_id, employee_id, year, month, remark.as_deref(), approval_request_id, now)
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
        employee_id: Uuid,
        year: i32,
        month: i32,
        approver_id: Option<Uuid>,
    ) -> Result<(), TimesheetError> {
        let now = Utc::now();
        let mut tx = self.scoped_tx().await?;

        let period = self
            .repo
            .period_row(&mut tx, employee_id, year, month)
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

        let hours = self.repo.sum_period_hours(&mut tx, employee_id, year, month).await?;
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
        employee_id: Uuid,
        year: i32,
        month: i32,
        remark: Option<String>,
    ) -> Result<(), TimesheetError> {
        let now = Utc::now();
        let mut tx = self.scoped_tx().await?;

        let period = self
            .repo
            .period_row(&mut tx, employee_id, year, month)
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
        employee_id: Uuid,
        date: NaiveDate,
    ) -> Result<(), TimesheetError> {
        self.assert_period_open_ym(conn, employee_id, date.year(), date.month() as i32)
            .await
    }

    async fn assert_period_open_ym(
        &self,
        conn: &mut sqlx::PgConnection,
        employee_id: Uuid,
        year: i32,
        month: i32,
    ) -> Result<(), TimesheetError> {
        if let Some(p) = self.repo.period_row(conn, employee_id, year, month).await? {
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

/// Map a DB error from the entry write path to the typed API error: the entries EXCLUDE
/// constraint (23P01) to the 409 overlap, and the invoiced-row guard trigger (which only a
/// racing stamp or a raw writer can trip — the service refuses earlier) to the 409 lock.
fn map_entry_write_error(e: sqlx::Error) -> TimesheetError {
    if let Some(db) = e.as_database_error() {
        if db.constraint().map(|c| c.contains("no_overlap")).unwrap_or(false) {
            return TimesheetError::EntryOverlap;
        }
        if db.message().contains("timesheet_invoiced_row_locked") {
            return TimesheetError::InvoicedRowLocked;
        }
    }
    TimesheetError::Db(e)
}
