//! Leave-regeneration probes — the PRJ-1 consumer half living in this module:
//! `regenerate_leave_rows` mirrors a settled timeoff request into analytic lines.
//!
//! Service-level probes (the verb has no HTTP surface by design — a HOST adapter consumes the
//! timeoff module's event sink, expands the window into per-day hours, and calls the verb).
//! Assertion reads are plain pool reads: the module is tenant-agnostic (ADR-0029 — no fence of
//! its own; a composing service's decorator owns isolation), and the probe DSN's role sees
//! through the leftover enable/force flags the strip deliberately left for the decorator to
//! own.
//!
//! Every write-driving body runs inside `org_scope::with_org_request_scope` with a
//! `for_company_unit` scope: the module relays the ambient scope onto its transactions, and
//! the company-keyed host seams (the rate lookup and the approvals filing) read the scope's
//! legacy company leg — which fails closed without a bound scope.
//!
//! DB: DATABASE_URL wins, else the module's local test DB. Fresh random
//! company/employee/request ids per test so parallel runs never collide.

use chrono::{Datelike, Duration, Months, NaiveDate, Utc};
use rust_decimal::Decimal;
use sqlx::PgPool;
use uuid::Uuid;

use backbone_timesheet::org_scope::{OrgScope, with_org_request_scope};
use backbone_timesheet::{
    LeaveDayEntry, LeaveRowSync, NewEntry, TimesheetError, TimesheetModule,
};

async fn pool() -> PgPool {
    let url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        "postgresql://serpa:serpa_dev_password@127.0.0.1:5432/backbone_timesheet_test".into()
    });
    PgPool::connect(&url).await.unwrap()
}

async fn module(pool: &PgPool) -> TimesheetModule {
    TimesheetModule::builder().with_database(pool.clone()).build().unwrap()
}

/// Plain scalar read for assertions.
async fn one<T>(pool: &PgPool, sql: String) -> T
where
    T: for<'r> sqlx::Decode<'r, sqlx::Postgres>
        + sqlx::Type<sqlx::Postgres>
        + Send
        + Sync
        + Unpin,
{
    sqlx::query_scalar::<_, T>(&sql).fetch_one(pool).await.unwrap()
}

/// Plain statement execution returning the sqlx Result, so probes can assert the DB-level
/// backstops (a raw duplicate insert raising 23505 on the leave-origin partial unique).
async fn exec(pool: &PgPool, sql: String) -> Result<sqlx::postgres::PgQueryResult, sqlx::Error> {
    sqlx::query(&sql).execute(pool).await
}

/// `(year, month, a safe in-month date)` for the PREVIOUS month — always complete, so the
/// submit window is always open for it.
fn prev_month() -> (i32, i32, NaiveDate) {
    // Derive the TRUE first of the previous month: subtracting Months from today keeps the
    // day-of-month (Aug 29 -> Jul 29), and "+5 days" would then spill into the current month,
    // landing entries in a different (year, month) than the period the probes lock.
    let today = Utc::now().date_naive();
    let first_of_this_month = NaiveDate::from_ymd_opt(today.year(), today.month(), 1).unwrap();
    let first_of_prev = first_of_this_month.checked_sub_months(Months::new(1)).unwrap();
    let (y, m) = (first_of_prev.year(), first_of_prev.month() as i32);
    (y, m, first_of_prev + Duration::days(5))
}

fn sync_for(
    employee: Uuid,
    request: Uuid,
    project: Uuid,
    days: Vec<(NaiveDate, i64)>,
) -> LeaveRowSync {
    LeaveRowSync {
        employee_id: employee,
        timeoff_request_id: request,
        project_id: project,
        task_id: None,
        remark: Some("annual leave".into()),
        entries: days
            .into_iter()
            .map(|(date, hours)| LeaveDayEntry { date, hours: Decimal::from(hours) })
            .collect(),
    }
}

/// One ordinary (non-leave) entry, written through the validated path.
async fn ordinary_entry(m: &TimesheetModule, employee: Uuid, date: NaiveDate) -> Uuid {
    m.timesheet_write_service
        .create_entry(NewEntry {
            employee_id: employee,
            project_id: Some(Uuid::new_v4()),
            task_id: None,
            date,
            remark: None,
            time_start: Some(date.and_hms_opt(9, 0, 0).unwrap().and_utc()),
            time_end: Some(date.and_hms_opt(17, 0, 0).unwrap().and_utc()),
            entry_type: "work",
            hours: None,
            activity_type_id: None,
            is_billable: None,
        })
        .await
        .expect("ordinary entry")
        .id
}

// ─── LV-1: regeneration is delete-and-regenerate, duplicate-proof ──────────────

async fn live_rows(pool: &PgPool, request: Uuid) -> i64 {
    one::<i64>(
        pool,
        format!(
            "SELECT count(*) FROM timesheet.timesheets WHERE source_timeoff_request_id = '{request}' AND (metadata->>'deleted_at') IS NULL"
        ),
    )
    .await
}

#[tokio::test]
async fn ts_leave_regenerate_no_duplicates() {
    let pool = pool().await;
    let m = module(&pool).await;
    let company = Uuid::new_v4();
    let employee = Uuid::new_v4();
    let project = Uuid::new_v4();
    let request = Uuid::new_v4();
    let (_, _, d1) = prev_month();
    let d2 = d1 + Duration::days(1);
    let d3 = d1 + Duration::days(2);
    let svc = &m.timesheet_write_service;

    with_org_request_scope(&pool, OrgScope::for_company_unit(company), async {
        // First regeneration: two days of 8h absence.
        let n = svc.regenerate_leave_rows(&sync_for(employee, request, project, vec![(d1, 8), (d2, 8)])).await.unwrap();
        assert_eq!(n, 2, "two rows inserted");
        assert_eq!(live_rows(&pool, request).await, 2);

        // Second regeneration of the SAME window: still two rows, never duplicates —
        // delete-and-regenerate, not append.
        let n = svc.regenerate_leave_rows(&sync_for(employee, request, project, vec![(d1, 8), (d2, 8)])).await.unwrap();
        assert_eq!(n, 2);
        assert_eq!(live_rows(&pool, request).await, 2, "regeneration never duplicates rows");
        let dead: i64 = one(&pool, format!(
            "SELECT count(*) FROM timesheet.timesheets WHERE source_timeoff_request_id = '{request}' AND (metadata->>'deleted_at') IS NOT NULL"
        )).await;
        assert_eq!(dead, 2, "the first generation was soft-deleted, not updated in place");

        // A changed window regenerates to exactly the new shape.
        let n = svc.regenerate_leave_rows(&sync_for(employee, request, project, vec![(d1, 4), (d2, 8), (d3, 8)])).await.unwrap();
        assert_eq!(n, 3);
        assert_eq!(live_rows(&pool, request).await, 3);
        let hours: Decimal = one(&pool, format!(
            "SELECT COALESCE(SUM(unit_amount), 0) FROM timesheet.timesheets WHERE source_timeoff_request_id = '{request}' AND (metadata->>'deleted_at') IS NULL"
        )).await;
        assert_eq!(hours, Decimal::from(20), "4 + 8 + 8");

        // The DB backstop: a raw duplicate live row on (request, day) cannot exist —
        // the partial unique raises 23505.
        let dup = exec(
            &pool,
            format!(
                r#"INSERT INTO timesheet.timesheets
                       (employee_id, project_id, year, month, date, entry_type,
                        unit_amount, source_timeoff_request_id, metadata)
                   VALUES ('{employee}', '{project}', {y}, {mo}, '{d1}', 'timeoff', 8, '{request}',
                           '{{"created_at":null,"updated_at":null,"deleted_at":null}}'::jsonb)"#,
                y = d1.year(),
                mo = d1.month() as i32,
            ),
        )
        .await;
        let err = dup.expect_err("a raw duplicate leave row must trip the partial unique");
        let hit = err
            .as_database_error()
            .map(|d| d.is_unique_violation() && d.constraint().map(|c| c.contains("leave_origin")).unwrap_or(false))
            .unwrap_or(false);
        assert!(hit, "expected 23505 on uq_timesheets_leave_origin, got: {err:?}");

        // An empty window (refused / cancelled / voided settlement) regenerates to zero rows.
        let n = svc.regenerate_leave_rows(&sync_for(employee, request, project, vec![])).await.unwrap();
        assert_eq!(n, 0);
        assert_eq!(live_rows(&pool, request).await, 0, "the leave's rows are gone");
    })
    .await
    .unwrap();
}

// ─── LV-2: authoritative over its OWN rows in a locked period — nothing else ───

#[tokio::test]
async fn ts_leave_regen_bypasses_lock_for_own_rows_only() {
    let pool = pool().await;
    let m = module(&pool).await;
    let company = Uuid::new_v4();
    let employee = Uuid::new_v4();
    let project = Uuid::new_v4();
    let request = Uuid::new_v4();
    let (y, mo, date) = prev_month();
    let svc = &m.timesheet_write_service;

    with_org_request_scope(&pool, OrgScope::for_company_unit(company), async {
        // An ordinary entry, then the period is submitted AND approved — locked.
        let ordinary = ordinary_entry(&m, employee, date).await;
        svc.submit_period(employee, y, mo, None, None).await.unwrap();
        svc.approve_period(employee, y, mo, None).await.unwrap();

        // Leave regeneration into the SAME (locked) period succeeds: the leave lifecycle rules
        // its OWN rows.
        let n = svc
            .regenerate_leave_rows(&sync_for(employee, request, project, vec![(date, 8)]))
            .await
            .expect("leave rows regenerate inside a locked period");
        assert_eq!(n, 1);

        // And it re-runs (idempotent re-delivery of the same settlement).
        let n = svc
            .regenerate_leave_rows(&sync_for(employee, request, project, vec![(date, 8)]))
            .await
            .expect("repeated regeneration is fine");
        assert_eq!(n, 1);

        // The period lock still guards ORDINARY rows: update, delete, and create all refuse.
        let err = svc
            .update_entry(
                ordinary,
                NewEntry {
                    employee_id: employee,
                    project_id: None,
                    task_id: None,
                    date,
                    remark: Some("touch".into()),
                    time_start: Some(date.and_hms_opt(9, 0, 0).unwrap().and_utc()),
                    time_end: Some(date.and_hms_opt(17, 0, 0).unwrap().and_utc()),
                    entry_type: "work",
                    hours: None,
                    activity_type_id: None,
                    is_billable: None,
                },
            )
            .await
            .expect_err("ordinary rows stay frozen");
        assert_eq!(err.code(), "period_locked");

        let err = svc.delete_entry(ordinary).await.expect_err("delete stays frozen");
        assert_eq!(err.code(), "period_locked");

        let err = svc
            .create_entry(NewEntry {
                employee_id: employee,
                project_id: None,
                task_id: None,
                date,
                remark: None,
                time_start: Some(date.and_hms_opt(18, 0, 0).unwrap().and_utc()),
                time_end: Some(date.and_hms_opt(19, 0, 0).unwrap().and_utc()),
                entry_type: "work",
                hours: None,
                activity_type_id: None,
                is_billable: None,
            })
            .await
            .expect_err("creating ordinary rows stays frozen");
        assert_eq!(err.code(), "period_locked");

        // The leave rows exist alongside the frozen ordinary row, correctly shaped.
        let shape: (String, i32, i32, Decimal) = one(&pool, format!(
            "SELECT (entry_type::text, year, month, unit_amount) FROM timesheet.timesheets WHERE source_timeoff_request_id = '{request}' AND (metadata->>'deleted_at') IS NULL"
        )).await;
        assert_eq!(shape, ("timeoff".into(), y, mo, Decimal::from(8)));
    })
    .await
    .unwrap();
}

// ─── LV-3: billed absence refuses loudly ───────────────────────────────────────

#[tokio::test]
async fn ts_leave_refuses_billed_row() {
    let pool = pool().await;
    let m = module(&pool).await;
    let company = Uuid::new_v4();
    let employee = Uuid::new_v4();
    let project = Uuid::new_v4();
    let request = Uuid::new_v4();
    let (_, _, d1) = prev_month();
    let d2 = d1 + Duration::days(1);
    let svc = &m.timesheet_write_service;

    with_org_request_scope(&pool, OrgScope::for_company_unit(company), async {
        svc.regenerate_leave_rows(&sync_for(employee, request, project, vec![(d1, 8), (d2, 8)]))
            .await
            .unwrap();

        // The billing exit stamped one of the leave days (a cross-module writer, simulated raw).
        exec(
            &pool,
            format!(
                "UPDATE timesheet.timesheets SET invoice_id = '{}' WHERE source_timeoff_request_id = '{request}' AND date = '{d1}' AND (metadata->>'deleted_at') IS NULL",
                Uuid::new_v4()
            ),
        )
        .await
        .unwrap();

        // Regeneration refuses: billed absence is corrected by reversing the invoice first.
        let err = svc
            .regenerate_leave_rows(&sync_for(employee, request, project, vec![(d1, 4), (d2, 8)]))
            .await
            .expect_err("billed absence must refuse loudly");
        assert_eq!(err.code(), "leave_row_billed");
        assert!(matches!(err, TimesheetError::LeaveRowBilled));

        // The refused run left the original rows untouched.
        let live: i64 = one(&pool, format!(
            "SELECT count(*) FROM timesheet.timesheets WHERE source_timeoff_request_id = '{request}' AND (metadata->>'deleted_at') IS NULL"
        )).await;
        assert_eq!(live, 2, "the refused regeneration wrote nothing");

        // After the reversal clears the link, regeneration proceeds.
        exec(
            &pool,
            format!(
                "UPDATE timesheet.timesheets SET invoice_id = NULL WHERE source_timeoff_request_id = '{request}' AND (metadata->>'deleted_at') IS NULL"
            ),
        )
        .await
        .unwrap();
        let n = svc
            .regenerate_leave_rows(&sync_for(employee, request, project, vec![(d1, 4), (d2, 8)]))
            .await
            .expect("regeneration proceeds once the invoice link is cleared");
        assert_eq!(n, 2);
    })
    .await
    .unwrap();
}
