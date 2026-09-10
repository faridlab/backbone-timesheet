//! Hand-written write SQL for the timesheet analytic line + period-approval flows.
//!
//! User-owned (declared in `metaphor.codegen.yaml`); the generator never touches it. Per the
//! 4-layer rule the SQL lives here, while [`crate::application::service::
//! timesheet_write_service::TimesheetWriteService`] owns the period-lock checks, the validation
//! window, the approvals seam, the rate-source seam, and the error mapping.
//!
//! Every method takes a `&mut PgConnection` (a transaction begun by the write service) — an
//! entry mutation and its period-lock read must observe one consistent snapshot, and a period
//! transition commits with its aggregate recompute or not at all. The caller MUST have relayed
//! the ambient org scope onto the connection (`org_scope::bind_org_scope_on`, when one is
//! bound) right after `begin()`.
//!
//! Tenancy (ADR-0029): the module is tenant-agnostic — no statement here names a tenant
//! column. Isolation is owned by the COMPOSING service: under its org request scope the
//! decorator's row-level fence applies; unfenced deployments run plain.
//!
//! Soft-delete lives in `metadata` JSONB (`deleted_at` key): every "live row" predicate is
//! `(metadata->>'deleted_at') IS NULL`, mirroring the module's partial indexes.
//!
//! Plain-amount posture (the converged analytic line): every rate/amount column below is
//! STORED. The service resolves rates on qualifying writes and hands the repo a fully stamped
//! [`EntryWrite`]; no SQL here computes an amount, and no read path reprices.

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use rust_decimal::Decimal;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::domain::entity::TimesheetType;

/// Every column of the analytic line the write paths return (the DTO shape).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct EntryRow {
    pub id: Uuid,
    pub employee_id: Uuid,
    pub project_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub date: NaiveDate,
    pub remark: Option<String>,
    pub time_start: Option<DateTime<Utc>>,
    pub time_end: Option<DateTime<Utc>>,
    pub entry_type: TimesheetType,
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

/// The row's stored state an update decision needs: period coordinates for the
/// lock, the invoice link for the write guard, and the rate/hours snapshot the
/// plain-amount rules keep when a write does not qualify for repricing.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct EntrySnapshot {
    pub employee_id: Uuid,
    pub year: i32,
    pub month: i32,
    pub invoice_id: Option<Uuid>,
    pub time_start: Option<DateTime<Utc>>,
    pub time_end: Option<DateTime<Utc>>,
    pub unit_amount: Decimal,
    pub activity_type_id: Option<Uuid>,
    pub billing_rate: Option<Decimal>,
    pub costing_rate: Option<Decimal>,
    pub is_billable: bool,
}

/// The live period-approval row (if any) for one employee-period.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PeriodRow {
    pub id: Uuid,
    /// Decoded as text — the service compares against the status vocabulary.
    pub status: String,
    pub approval_request_id: Option<Uuid>,
}

/// New-entry payload shared by create and update. The service derives the plain-stored
/// figures (`unit_amount` from the windows or the explicit hours input; rates through the
/// rate-source port on qualifying writes) and hands the repo a stamped [`EntryWrite`].
#[derive(Debug, Clone)]
pub struct NewEntry {
    pub employee_id: Uuid,
    pub project_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub date: NaiveDate,
    pub remark: Option<String>,
    pub time_start: Option<DateTime<Utc>>,
    pub time_end: Option<DateTime<Utc>>,
    pub entry_type: &'static str,
    /// Explicit hours for a duration-only entry (used when the windows are absent).
    pub hours: Option<Decimal>,
    /// The row's activity classification — a rate-determining field.
    pub activity_type_id: Option<Uuid>,
    /// Billability FLAG (defaults to true on create, keeps the stored flag when absent on update).
    pub is_billable: Option<bool>,
}

/// A fully stamped entry write: the plain-stored hours + rate/amount snapshots the service
/// resolved. `year`/`month` derive from `date`; the table columns stay denormalized for the
/// period queries.
#[derive(Debug, Clone)]
pub struct EntryWrite {
    pub project_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub date: NaiveDate,
    pub remark: Option<String>,
    pub time_start: Option<DateTime<Utc>>,
    pub time_end: Option<DateTime<Utc>>,
    pub entry_type: &'static str,
    pub unit_amount: Decimal,
    pub activity_type_id: Option<Uuid>,
    pub billing_rate: Option<Decimal>,
    pub costing_rate: Option<Decimal>,
    pub is_billable: bool,
    pub billable_amount: Decimal,
    pub costing_amount: Decimal,
}

/// The column list every entry RETURNING shares.
const ENTRY_COLUMNS: &str = "id, employee_id, project_id, task_id, date, remark, time_start, time_end, entry_type, \
                             unit_amount, currency, activity_type_id, billing_rate, costing_rate, is_billable, \
                             billable_amount, costing_amount, invoice_id, source_timeoff_request_id";

/// One day of a leave regeneration: the date and the plain-stored hours for that day.
#[derive(Debug, Clone)]
pub struct LeaveDayEntry {
    pub date: NaiveDate,
    pub hours: Decimal,
}

/// A leave regeneration request: delete the request's live rows, insert one row per day.
#[derive(Debug, Clone)]
pub struct LeaveRowSync {
    pub employee_id: Uuid,
    pub timeoff_request_id: Uuid,
    pub project_id: Uuid,
    pub task_id: Option<Uuid>,
    pub remark: Option<String>,
    pub entries: Vec<LeaveDayEntry>,
}

pub struct TimesheetWriteRepository;

impl TimesheetWriteRepository {
    // ─── period state (the lock) ───────────────────────────────────────────────

    /// The employee's live period row, if one exists (any status). `status` comes back as text:
    /// `pending`/`approved` freeze the period's entries, `rejected` reopens it for edits.
    pub async fn period_row(
        &self,
        conn: &mut PgConnection,
        employee_id: Uuid,
        year: i32,
        month: i32,
    ) -> Result<Option<PeriodRow>, sqlx::Error> {
        sqlx::query_as::<_, PeriodRow>(
            r#"SELECT id, status::text AS status, approval_request_id
                 FROM timesheet.timesheet_approvals
                WHERE employee_id = $1 AND year = $2 AND month = $3
                  AND (metadata->>'deleted_at') IS NULL"#,
        )
        .bind(employee_id)
        .bind(year)
        .bind(month)
        .fetch_optional(conn)
        .await
    }

    /// Live entries in the period — the submit gate (`EmptyPeriod`) and the approve-time
    /// billable aggregate both read from here.
    pub async fn live_entry_count(
        &self,
        conn: &mut PgConnection,
        employee_id: Uuid,
        year: i32,
        month: i32,
    ) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar::<_, i64>(
            r#"SELECT count(*) FROM timesheet.timesheets
                WHERE employee_id = $1 AND year = $2 AND month = $3
                  AND (metadata->>'deleted_at') IS NULL"#,
        )
        .bind(employee_id)
        .bind(year)
        .bind(month)
        .fetch_one(conn)
        .await
    }

    /// Total logged hours for the period — a plain SUM over the stored `unit_amount` (the
    /// converged hours figure: ranged rows carry their window-derived hours, duration-only
    /// rows their explicit input). One decimal hour figure, the number the filing shows the
    /// approver and what approve stamps as `billable_time`. Never recomputed from windows.
    pub async fn sum_period_hours(
        &self,
        conn: &mut PgConnection,
        employee_id: Uuid,
        year: i32,
        month: i32,
    ) -> Result<Decimal, sqlx::Error> {
        sqlx::query_scalar::<_, Decimal>(
            r#"SELECT COALESCE(SUM(unit_amount), 0)::numeric(18,2)
                 FROM timesheet.timesheets
                WHERE employee_id = $1 AND year = $2 AND month = $3
                  AND (metadata->>'deleted_at') IS NULL"#,
        )
        .bind(employee_id)
        .bind(year)
        .bind(month)
        .fetch_one(conn)
        .await
    }

    // ─── entries ────────────────────────────────────────────────────────────────

    /// The row's stored state for update decisions (period coordinates, invoice link, and the
    /// plain-stored snapshot). Reads without repricing — the plain-amount rule.
    pub async fn entry_snapshot(
        &self,
        conn: &mut PgConnection,
        entry_id: Uuid,
    ) -> Result<Option<EntrySnapshot>, sqlx::Error> {
        sqlx::query_as::<_, EntrySnapshot>(
            r#"SELECT employee_id, year, month, invoice_id, time_start, time_end, unit_amount,
                      activity_type_id, billing_rate, costing_rate, is_billable
                 FROM timesheet.timesheets
                WHERE id = $1
                  AND (metadata->>'deleted_at') IS NULL"#,
        )
        .bind(entry_id)
        .fetch_optional(conn)
        .await
    }

    /// Insert an entry with its stamped plain-stored figures. The `timesheets_no_overlap`
    /// EXCLUDE constraint is the arbiter for ranged rows — a clashing insert lands here as
    /// 23P01 and maps to `EntryOverlap` upstream. Duration-only rows (NULL bounds) are exempt
    /// by the constraint's predicate.
    pub async fn insert_entry(
        &self,
        conn: &mut PgConnection,
        employee_id: Uuid,
        w: &EntryWrite,
        now: DateTime<Utc>,
    ) -> Result<EntryRow, sqlx::Error> {
        // employee_id is an INSERT-only column: updates never move a row between
        // employees (the row's employee is the resolution itself).
        sqlx::query_as::<_, EntryRow>(
            &format!(r#"INSERT INTO timesheet.timesheets
                   (id, employee_id, project_id, task_id, year, month, date,
                    remark, time_start, time_end, entry_type,
                    unit_amount, activity_type_id, billing_rate, costing_rate, is_billable,
                    billable_amount, costing_amount, metadata)
               VALUES (gen_random_uuid(), $1, $2, $3, $4, $5, $6, $7, $8, $9, $10::timesheet_type,
                       $11, $12, $13, $14, $15, $16, $17,
                       jsonb_build_object('created_at', to_jsonb($18::timestamptz),
                                          'updated_at', to_jsonb($18::timestamptz)))
               RETURNING {ENTRY_COLUMNS}"#),
        )
        .bind(employee_id)
        .bind(w.project_id)
        .bind(w.task_id)
        .bind(w.date.year())
        .bind(w.date.month() as i32)
        .bind(w.date)
        .bind(&w.remark)
        .bind(w.time_start)
        .bind(w.time_end)
        .bind(w.entry_type)
        .bind(w.unit_amount)
        .bind(w.activity_type_id)
        .bind(w.billing_rate)
        .bind(w.costing_rate)
        .bind(w.is_billable)
        .bind(w.billable_amount)
        .bind(w.costing_amount)
        .bind(now)
        .fetch_one(conn)
        .await
    }

    /// Write an entry's mutable columns with their stamped figures (full update — the service
    /// validates the whole patch and resolves what the plain-amount rules re-stamp). The
    /// EXCLUDE constraint re-validates the new range against every other live entry, and the
    /// invoiced-row guard trigger backstops writes the service already refused.
    #[allow(clippy::too_many_arguments)]
    pub async fn update_entry(
        &self,
        conn: &mut PgConnection,
        entry_id: Uuid,
        w: &EntryWrite,
        now: DateTime<Utc>,
    ) -> Result<Option<EntryRow>, sqlx::Error> {
        sqlx::query_as::<_, EntryRow>(
            &format!(r#"UPDATE timesheet.timesheets
                  SET project_id = $2, task_id = $3, year = $4, month = $5, date = $6,
                      remark = $7, time_start = $8, time_end = $9,
                      entry_type = $10::timesheet_type,
                      unit_amount = $11, activity_type_id = $12,
                      billing_rate = $13, costing_rate = $14, is_billable = $15,
                      billable_amount = $16, costing_amount = $17,
                      metadata = metadata || jsonb_build_object('updated_at', to_jsonb($18::timestamptz))
                WHERE id = $1
                  AND (metadata->>'deleted_at') IS NULL
                RETURNING {ENTRY_COLUMNS}"#),
        )
        .bind(entry_id)
        .bind(w.project_id)
        .bind(w.task_id)
        .bind(w.date.year())
        .bind(w.date.month() as i32)
        .bind(w.date)
        .bind(&w.remark)
        .bind(w.time_start)
        .bind(w.time_end)
        .bind(w.entry_type)
        .bind(w.unit_amount)
        .bind(w.activity_type_id)
        .bind(w.billing_rate)
        .bind(w.costing_rate)
        .bind(w.is_billable)
        .bind(w.billable_amount)
        .bind(w.costing_amount)
        .bind(now)
        .fetch_optional(conn)
        .await
    }

    /// Soft-delete an entry (period must be open and the row unbilled — checked by the service first).
    pub async fn soft_delete_entry(
        &self,
        conn: &mut PgConnection,
        entry_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(
            r#"UPDATE timesheet.timesheets
                  SET metadata = metadata || jsonb_build_object('deleted_at', to_jsonb($2::timestamptz))
                WHERE id = $1
                  AND (metadata->>'deleted_at') IS NULL"#,
        )
        .bind(entry_id)
        .bind(now)
        .execute(conn)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    // ─── leave regeneration (delete-and-regenerate, origin-fenced) ─────────────

    /// Live rows of one leave request that already carried an invoice link — billed absence
    /// is a loud operator case, never silently regenerated away.
    pub async fn count_billed_leave_rows(
        &self,
        conn: &mut PgConnection,
        timeoff_request_id: Uuid,
    ) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar::<_, i64>(
            r#"SELECT count(*) FROM timesheet.timesheets
                WHERE source_timeoff_request_id = $1
                  AND invoice_id IS NOT NULL
                  AND (metadata->>'deleted_at') IS NULL"#,
        )
        .bind(timeoff_request_id)
        .fetch_one(conn)
        .await
    }

    /// Soft-delete the request's live rows — the first half of regeneration. Runs inside the
    /// regeneration tx, so the partial unique `(source_timeoff_request_id, date)`
    /// sees delete+insert as one step and a raw duplicate insert cannot survive it.
    pub async fn soft_delete_leave_rows(
        &self,
        conn: &mut PgConnection,
        timeoff_request_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<u64, sqlx::Error> {
        let res = sqlx::query(
            r#"UPDATE timesheet.timesheets
                  SET metadata = metadata || jsonb_build_object('deleted_at', to_jsonb($2::timestamptz))
                WHERE source_timeoff_request_id = $1
                  AND (metadata->>'deleted_at') IS NULL"#,
        )
        .bind(timeoff_request_id)
        .bind(now)
        .execute(conn)
        .await?;
        Ok(res.rows_affected())
    }

    /// Insert one leave-regenerated day row: `entry_type='timeoff'`, hours as the plain-stored
    /// `unit_amount`, rates NULL (no rate source applies to absence) so amounts are 0, year and
    /// month derived from the date. The partial unique on the origin key is the no-duplicates
    /// backstop.
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_leave_row(
        &self,
        conn: &mut PgConnection,
        sync: &LeaveRowSync,
        day: &LeaveDayEntry,
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"INSERT INTO timesheet.timesheets
                   (id, employee_id, project_id, task_id, year, month, date,
                    remark, entry_type, unit_amount, source_timeoff_request_id, metadata)
               VALUES (gen_random_uuid(), $1, $2, $3, $4, $5, $6, $7, 'timeoff', $8, $9,
                       jsonb_build_object('created_at', to_jsonb($10::timestamptz),
                                          'updated_at', to_jsonb($10::timestamptz)))"#,
        )
        .bind(sync.employee_id)
        .bind(sync.project_id)
        .bind(sync.task_id)
        .bind(day.date.year())
        .bind(day.date.month() as i32)
        .bind(day.date)
        .bind(sync.remark.as_deref())
        .bind(day.hours)
        .bind(sync.timeoff_request_id)
        .bind(now)
        .execute(conn)
        .await?;
        Ok(())
    }

    // ─── period transitions ────────────────────────────────────────────────────

    /// Create the period row `pending` (first submit). Id is supplied by the service so the
    /// approvals filing (which happens BEFORE the tx, timeoff's file-first ordering) can carry
    /// the correlation id.
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_period_pending(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
        employee_id: Uuid,
        year: i32,
        month: i32,
        remark: Option<&str>,
        approval_request_id: Option<Uuid>,
        submitted_at: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"INSERT INTO timesheet.timesheet_approvals
                   (id, employee_id, year, month, remark, status,
                    approval_request_id, submitted_at, metadata)
               VALUES ($1, $2, $3, $4, $5, 'pending', $6, $7,
                       jsonb_build_object('created_at', to_jsonb($7::timestamptz),
                                          'updated_at', to_jsonb($7::timestamptz)))"#,
        )
        .bind(id)
        .bind(employee_id)
        .bind(year)
        .bind(month)
        .bind(remark)
        .bind(approval_request_id)
        .bind(submitted_at)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// Revive a rejected period back to pending (re-submit after rejection). Replaces the
    /// approval link — a new cycle files a new engine request.
    pub async fn revive_period_pending(
        &self,
        conn: &mut PgConnection,
        period_id: Uuid,
        remark: Option<&str>,
        approval_request_id: Option<Uuid>,
        submitted_at: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"UPDATE timesheet.timesheet_approvals
                  SET status = 'pending',
                      remark = COALESCE($2, remark),
                      approval_request_id = $3,
                      submitted_at = $4,
                      metadata = metadata || jsonb_build_object('updated_at', to_jsonb($4::timestamptz))
                WHERE id = $1
                  AND status = 'rejected'
                  AND (metadata->>'deleted_at') IS NULL"#,
        )
        .bind(period_id)
        .bind(remark)
        .bind(approval_request_id)
        .bind(submitted_at)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// pending → approved, stamping the approver and the billable aggregate computed under the
    /// same tx. The `WHERE status = 'pending'` guard makes a racing double-approve a 0-row
    /// update the service reports as `NotPending`.
    pub async fn mark_period_approved(
        &self,
        conn: &mut PgConnection,
        period_id: Uuid,
        approver_id: Option<Uuid>,
        billable_time: Decimal,
        now: DateTime<Utc>,
    ) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(
            r#"UPDATE timesheet.timesheet_approvals
                  SET status = 'approved',
                      approver_id = $2,
                      billable_time = $3,
                      metadata = metadata || jsonb_build_object('updated_at', to_jsonb($4::timestamptz))
                WHERE id = $1
                  AND status = 'pending'
                  AND (metadata->>'deleted_at') IS NULL"#,
        )
        .bind(period_id)
        .bind(approver_id)
        .bind(billable_time)
        .bind(now)
        .execute(conn)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    /// pending → rejected — the period reopens for edits (the lock only freezes pending/approved).
    pub async fn mark_period_rejected(
        &self,
        conn: &mut PgConnection,
        period_id: Uuid,
        remark: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(
            r#"UPDATE timesheet.timesheet_approvals
                  SET status = 'rejected',
                      remark = COALESCE($2, remark),
                      metadata = metadata || jsonb_build_object('updated_at', to_jsonb($3::timestamptz))
                WHERE id = $1
                  AND status = 'pending'
                  AND (metadata->>'deleted_at') IS NULL"#,
        )
        .bind(period_id)
        .bind(remark)
        .bind(now)
        .execute(conn)
        .await?;
        Ok(res.rows_affected() > 0)
    }
}
