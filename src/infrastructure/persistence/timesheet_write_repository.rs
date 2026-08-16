//! Hand-written write SQL for the timesheet entry + period-approval flows (H-6).
//!
//! User-owned (declared in `metaphor.codegen.yaml`); the generator never touches it. Per the
//! 4-layer rule the SQL lives here, while [`crate::application::service::
//! timesheet_write_service::TimesheetWriteService`] owns the period-lock checks, the validation
//! window, the approvals seam, and the error mapping.
//!
//! Every method takes a `&mut PgConnection` (a transaction begun by the write service) — an
//! entry mutation and its period-lock read must observe one consistent snapshot, and a period
//! transition commits with its aggregate recompute or not at all. The caller MUST have bound the
//! company onto the connection (`company_scope::bind_company_on`) right after `begin()`.
//!
//! Soft-delete lives in `metadata` JSONB (`deleted_at` key): every "live row" predicate is
//! `(metadata->>'deleted_at') IS NULL`, mirroring the module's partial indexes and the fence.

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use rust_decimal::Decimal;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::domain::entity::TimesheetType;

/// One entry row (create/update return shape). `year`/`month` are derived from `date` by the
/// service; the table columns stay denormalized for the period queries below.
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
}

/// The live period-approval row (if any) for one employee-period.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PeriodRow {
    pub id: Uuid,
    /// Decoded as text — the service compares against the status vocabulary.
    pub status: String,
    pub approval_request_id: Option<Uuid>,
}

/// New-entry payload shared by create and update (update replaces every mutable column).
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
}

pub struct TimesheetWriteRepository;

impl TimesheetWriteRepository {
    // ─── period state (the lock) ───────────────────────────────────────────────

    /// The employee's live period row, if one exists (any status). `status` comes back as text:
    /// `pending`/`approved` freeze the period's entries, `rejected` reopens it for edits.
    pub async fn period_row(
        &self,
        conn: &mut PgConnection,
        company_id: Uuid,
        employee_id: Uuid,
        year: i32,
        month: i32,
    ) -> Result<Option<PeriodRow>, sqlx::Error> {
        sqlx::query_as::<_, PeriodRow>(
            r#"SELECT id, status::text AS status, approval_request_id
                 FROM timesheet.timesheet_approvals
                WHERE company_id = $1 AND employee_id = $2 AND year = $3 AND month = $4
                  AND (metadata->>'deleted_at') IS NULL"#,
        )
        .bind(company_id)
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
        company_id: Uuid,
        employee_id: Uuid,
        year: i32,
        month: i32,
    ) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar::<_, i64>(
            r#"SELECT count(*) FROM timesheet.timesheets
                WHERE company_id = $1 AND employee_id = $2 AND year = $3 AND month = $4
                  AND (metadata->>'deleted_at') IS NULL"#,
        )
        .bind(company_id)
        .bind(employee_id)
        .bind(year)
        .bind(month)
        .fetch_one(conn)
        .await
    }

    /// Total logged hours for the period (summed over ranged entries; duration-only rows have
    /// no bounds and contribute nothing until ranged). One decimal hour figure, the number the
    /// filing shows the approver and what approve stamps as `billable_time`.
    pub async fn sum_period_hours(
        &self,
        conn: &mut PgConnection,
        company_id: Uuid,
        employee_id: Uuid,
        year: i32,
        month: i32,
    ) -> Result<Decimal, sqlx::Error> {
        sqlx::query_scalar::<_, Decimal>(
            r#"SELECT COALESCE(SUM(EXTRACT(EPOCH FROM (time_end - time_start)) / 3600.0), 0)::numeric
                 FROM timesheet.timesheets
                WHERE company_id = $1 AND employee_id = $2 AND year = $3 AND month = $4
                  AND time_start IS NOT NULL AND time_end IS NOT NULL
                  AND (metadata->>'deleted_at') IS NULL"#,
        )
        .bind(company_id)
        .bind(employee_id)
        .bind(year)
        .bind(month)
        .fetch_one(conn)
        .await
    }

    // ─── entries ────────────────────────────────────────────────────────────────

    /// Insert an entry. The `timesheets_no_overlap` EXCLUDE constraint is the arbiter for ranged
    /// rows — a clashing insert lands here as 23P01 and maps to `EntryOverlap` upstream.
    /// Duration-only rows (NULL bounds) are exempt by the constraint's predicate.
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_entry(
        &self,
        conn: &mut PgConnection,
        company_id: Uuid,
        e: &NewEntry,
        now: DateTime<Utc>,
    ) -> Result<EntryRow, sqlx::Error> {
        sqlx::query_as::<_, EntryRow>(
            r#"INSERT INTO timesheet.timesheets
                   (id, company_id, employee_id, project_id, task_id, year, month, date,
                    remark, time_start, time_end, entry_type, metadata)
               VALUES (gen_random_uuid(), $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11::timesheet_type,
                       jsonb_build_object('created_at', to_jsonb($12::timestamptz),
                                          'updated_at', to_jsonb($12::timestamptz)))
               RETURNING id, employee_id, project_id, task_id, date, remark, time_start, time_end, entry_type"#,
        )
        .bind(company_id)
        .bind(e.employee_id)
        .bind(e.project_id)
        .bind(e.task_id)
        .bind(e.date.year())
        .bind(e.date.month() as i32)
        .bind(e.date)
        .bind(&e.remark)
        .bind(e.time_start)
        .bind(e.time_end)
        .bind(e.entry_type)
        .bind(now)
        .fetch_one(conn)
        .await
    }

    /// Replace an entry's mutable columns (full update — the service validates the whole patch).
    /// The EXCLUDE constraint re-validates the new range against every other live entry.
    #[allow(clippy::too_many_arguments)]
    pub async fn update_entry(
        &self,
        conn: &mut PgConnection,
        company_id: Uuid,
        entry_id: Uuid,
        e: &NewEntry,
        now: DateTime<Utc>,
    ) -> Result<Option<EntryRow>, sqlx::Error> {
        sqlx::query_as::<_, EntryRow>(
            r#"UPDATE timesheet.timesheets
                  SET project_id = $3, task_id = $4, year = $5, month = $6, date = $7,
                      remark = $8, time_start = $9, time_end = $10,
                      entry_type = $11::timesheet_type,
                      metadata = metadata || jsonb_build_object('updated_at', to_jsonb($12::timestamptz))
                WHERE company_id = $1 AND id = $2
                  AND (metadata->>'deleted_at') IS NULL
                RETURNING id, employee_id, project_id, task_id, date, remark, time_start, time_end, entry_type"#,
        )
        .bind(company_id)
        .bind(entry_id)
        .bind(e.project_id)
        .bind(e.task_id)
        .bind(e.date.year())
        .bind(e.date.month() as i32)
        .bind(e.date)
        .bind(&e.remark)
        .bind(e.time_start)
        .bind(e.time_end)
        .bind(e.entry_type)
        .bind(now)
        .fetch_optional(conn)
        .await
    }

    /// Soft-delete an entry (period must be open — checked by the service first).
    pub async fn soft_delete_entry(
        &self,
        conn: &mut PgConnection,
        company_id: Uuid,
        entry_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(
            r#"UPDATE timesheet.timesheets
                  SET metadata = metadata || jsonb_build_object('deleted_at', to_jsonb($3::timestamptz))
                WHERE company_id = $1 AND id = $2
                  AND (metadata->>'deleted_at') IS NULL"#,
        )
        .bind(company_id)
        .bind(entry_id)
        .bind(now)
        .execute(conn)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    /// The entry's (employee, year, month) — so the delete path can check the period lock
    /// without trusting a client-supplied period.
    pub async fn entry_period(
        &self,
        conn: &mut PgConnection,
        company_id: Uuid,
        entry_id: Uuid,
    ) -> Result<Option<(Uuid, i32, i32)>, sqlx::Error> {
        sqlx::query_as::<_, (Uuid, i32, i32)>(
            r#"SELECT employee_id, year, month FROM timesheet.timesheets
                WHERE company_id = $1 AND id = $2
                  AND (metadata->>'deleted_at') IS NULL"#,
        )
        .bind(company_id)
        .bind(entry_id)
        .fetch_optional(conn)
        .await
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
        company_id: Uuid,
        employee_id: Uuid,
        year: i32,
        month: i32,
        remark: Option<&str>,
        approval_request_id: Option<Uuid>,
        submitted_at: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"INSERT INTO timesheet.timesheet_approvals
                   (id, company_id, employee_id, year, month, remark, status,
                    approval_request_id, submitted_at, metadata)
               VALUES ($1, $2, $3, $4, $5, $6, 'pending', $7, $8,
                       jsonb_build_object('created_at', to_jsonb($8::timestamptz),
                                          'updated_at', to_jsonb($8::timestamptz)))"#,
        )
        .bind(id)
        .bind(company_id)
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

