//! Integrity probes — route-level (Wave 1 P2, H-6). The guarded composition locks generic
//! mutation, the period lock and submit window hold, the EXCLUDE overlap surfaces as 409, the
//! approvals seam fails closed (TR2), and the company fence holds cross-tenant.
//!
//! Every request runs behind the REAL `company_auth` middleware with a minted HS256 token —
//! the same mounting a composing service uses in production (the party probe-suite harness
//! pattern). The DB runs the strict fence (RLS ENABLE+FORCE on every timesheet table), so even
//! this owner-role connection is fenced: raw assertion SQL runs inside
//! `company_scope::with_company_scope` (re-exported at the crate root for this suite).
//!
//! DB: DATABASE_URL wins, else the module's local test DB (`backbone_timesheet_test` on the
//! metaphora dev postgres). Fresh random company/employee ids per test so parallel runs never
//! collide.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::from_fn_with_state;
use chrono::{DateTime, Datelike, Duration, Months, NaiveDate, Utc};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use backbone_auth::company::{company_auth, CompanyVerifier};
use backbone_timesheet::{
    create_guarded_timesheet_routes, company_scope, TimesheetFiling, TimesheetFilingRequest,
    TimesheetModule, TimesheetSeamError, TimesheetVerdict,
};

const SECRET: &[u8] = b"timesheet-integrity-probe-secret";

async fn pool() -> PgPool {
    let url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        "postgresql://serpa:serpa_dev_password@127.0.0.1:5432/backbone_timesheet_test".into()
    });
    PgPool::connect(&url).await.unwrap()
}

async fn module(pool: &PgPool) -> TimesheetModule {
    TimesheetModule::builder().with_database(pool.clone()).build().unwrap()
}

fn token_for(company: Uuid) -> String {
    let exp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as usize
        + 3600;
    let claims = serde_json::json!({"sub": "integrity-probe", "company_id": company, "exp": exp});
    jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(SECRET),
    ).unwrap()
}

/// One request through the REAL auth layer; returns status + response body (ids come back in it).
async fn req_full(
    app: axum::Router,
    method: &str,
    uri: &str,
    token: &str,
    body: String,
) -> (StatusCode, serde_json::Value) {
    let app = app.route_layer(from_fn_with_state(
        CompanyVerifier::hs256(SECRET),
        company_auth,
    ));
    let r = Request::builder().method(method).uri(uri)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(body)).unwrap();
    let resp = app.oneshot(r).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

async fn req(
    app: axum::Router,
    method: &str,
    uri: &str,
    token: &str,
    body: String,
) -> StatusCode {
    req_full(app, method, uri, token, body).await.0
}

/// Scoped scalar read for assertions — binds `app.company_id` the way the request scope does
/// so the FORCE-fenced tables answer under RLS (an unbound connection sees 0 rows by design).
/// The SQL embeds employee/period filters inline; the company filter IS the fence.
async fn scoped_one<T>(pool: &PgPool, company: Uuid, sql: String) -> T
where
    T: for<'r> sqlx::Decode<'r, sqlx::Postgres>
        + sqlx::Type<sqlx::Postgres>
        + Send
        + Sync
        + Unpin,
{
    company_scope::with_company_scope(Some(company), async move {
        sqlx::query_scalar::<_, T>(&sql).fetch_one(pool).await.unwrap()
    }).await
}

// ─── period fixtures ───────────────────────────────────────────────────────────

/// `(year, month, a safe in-month date)` for the PREVIOUS month — always complete, so the
/// submit window is always open for it (today is past its last day by construction).
fn prev_month() -> (i32, i32, NaiveDate) {
    let first_of_prev = Utc::now().date_naive().checked_sub_months(Months::new(1)).unwrap();
    let (y, m) = (first_of_prev.year(), first_of_prev.month() as i32);
    (y, m, first_of_prev + Duration::days(5)) // day 6 — inside every month
}

fn at(date: NaiveDate, h: u32) -> DateTime<Utc> {
    date.and_hms_opt(h, 0, 0).unwrap().and_utc()
}

/// Body for a ranged entry on `date` with the given hour bounds.
fn entry_body(employee: Uuid, date: NaiveDate, start_h: u32, end_h: u32) -> String {
    format!(
        r#"{{"employeeId":"{employee}","date":"{date}","timeStart":"{}","timeEnd":"{}"}}"#,
        at(date, start_h).to_rfc3339(),
        at(date, end_h).to_rfc3339(),
    )
}

async fn create_entry(app: axum::Router, t: &str, employee: Uuid, date: NaiveDate) -> (StatusCode, Uuid) {
    let (s, j) = req_full(app, "POST", "/timesheets/entries", t, entry_body(employee, date, 9, 17)).await;
    let id = j.get("id").and_then(|v| v.as_str()).and_then(|s| Uuid::parse_str(s).ok());
    (s, id.unwrap_or_default())
}

// ─── a controllable approvals port (TR2 probes) ────────────────────────────────

/// In-test port: filing always succeeds with a fresh request id; the verdict is mutable so a
/// test can walk a period through the engine's decision states.
struct StubApprovals {
    verdict: std::sync::Mutex<TimesheetVerdict>,
}

#[async_trait::async_trait]
impl TimesheetFiling for StubApprovals {
    async fn file(&self, _req: &TimesheetFilingRequest) -> Result<Uuid, TimesheetSeamError> {
        Ok(Uuid::new_v4())
    }
    async fn status(&self, _id: Uuid) -> Result<TimesheetVerdict, TimesheetSeamError> {
        Ok(*self.verdict.lock().unwrap())
    }
}

// ─── TS-1: entry lifecycle + range validation ──────────────────────────────────

#[tokio::test]
async fn guarded_entry_create_and_invalid_range() {
    let pool = pool().await;
    let m = module(&pool).await;
    let company = Uuid::new_v4();
    let employee = Uuid::new_v4();
    let t = token_for(company);
    let (_, _, date) = prev_month();

    let (s, id) = create_entry(create_guarded_timesheet_routes(&m), &t, employee, date).await;
    assert_eq!(s, StatusCode::CREATED, "ranged entry create");

    // end <= start is refused before any SQL runs.
    let bad = format!(
        r#"{{"employeeId":"{employee}","date":"{date}","timeStart":"{}","timeEnd":"{}"}}"#,
        at(date, 17).to_rfc3339(), at(date, 9).to_rfc3339(),
    );
    let s = req(create_guarded_timesheet_routes(&m), "POST", "/timesheets/entries", &t, bad).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "reversed bounds must be 422");

    // Bad entryType vocabulary too.
    let bad_type = format!(
        r#"{{"employeeId":"{employee}","date":"{date}","entryType":"nonsense"}}"#
    );
    let s = req(create_guarded_timesheet_routes(&m), "POST", "/timesheets/entries", &t, bad_type).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "garbage entryType must be 422");

    // And the row is live under the fence.
    let n: i64 = scoped_one(&pool, company, format!(
        "SELECT count(*) FROM timesheet.timesheets WHERE employee_id = '{employee}'"
    )).await;
    assert_eq!(n, 1, "exactly the one live entry (bad creates wrote nothing)");
    assert_ne!(id, Uuid::default(), "create returns the entry id");
}

// ─── TS-2: EXCLUDE overlap surfaces as 409; duration-only rows never clash ─────

#[tokio::test]
async fn guarded_entry_overlap_rejected_by_exclude() {
    let pool = pool().await;
    let m = module(&pool).await;
    let company = Uuid::new_v4();
    let employee = Uuid::new_v4();
    let t = token_for(company);
    let (_, _, date) = prev_month();
    let app = create_guarded_timesheet_routes(&m);

    let (s, _) = create_entry(app.clone(), &t, employee, date).await;
    assert_eq!(s, StatusCode::CREATED, "first entry 09:00–17:00");

    // 10:00–11:00 sits inside [09:00, 17:00) — the constraint is the arbiter.
    let overlapping = entry_body(employee, date, 10, 11);
    let s = req(app.clone(), "POST", "/timesheets/entries", &t, overlapping).await;
    assert_eq!(s, StatusCode::CONFLICT, "overlapping entry must be 409 entry_overlap");

    // Adjacent is fine; duration-only (no bounds) is exempt by the constraint's predicate.
    let adjacent = entry_body(employee, date, 17, 18);
    let s = req(app.clone(), "POST", "/timesheets/entries", &t, adjacent).await;
    assert_eq!(s, StatusCode::CREATED, "adjacent entry is allowed");

    let draft1 = format!(r#"{{"employeeId":"{employee}","date":"{date}"}}"#);
    let s = req(app.clone(), "POST", "/timesheets/entries", &t, draft1.clone()).await;
    assert_eq!(s, StatusCode::CREATED, "duration-only draft #1");
    let s = req(app, "POST", "/timesheets/entries", &t, draft1).await;
    assert_eq!(s, StatusCode::CREATED, "duration-only draft #2 — no bounds, no clash");
}

// ─── TS-3: the submit validation window ────────────────────────────────────────

#[tokio::test]
async fn guarded_submit_window_gates_current_month() {
    let pool = pool().await;
    let m = module(&pool).await;
    let company = Uuid::new_v4();
    let employee = Uuid::new_v4();
    let t = token_for(company);
    let app = create_guarded_timesheet_routes(&m);

    // The CURRENT month is still in progress — no submitting it, entries or not.
    let today = Utc::now().date_naive();
    let current = format!(
        r#"{{"employeeId":"{employee}","year":{},"month":{}}}"#,
        today.year(), today.month() as i32
    );
    let s = req(app.clone(), "POST", "/timesheets/periods/submit", &t, current).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "in-progress month must be 422 window_not_open");

    // The previous month is complete: an entry in it submits cleanly.
    let (y, mo, date) = prev_month();
    let (s, _) = create_entry(app.clone(), &t, employee, date).await;
    assert_eq!(s, StatusCode::CREATED, "seed entry");
    let past = format!(r#"{{"employeeId":"{employee}","year":{y},"month":{mo}}}"#);
    let (s, _) = req_full(app, "POST", "/timesheets/periods/submit", &t, past).await;
    assert_eq!(s, StatusCode::CREATED, "complete month submits");
}

// ─── TS-4: the period lock freezes entries while pending AND approved ──────────

#[tokio::test]
async fn guarded_period_lock_freezes_entries() {
    let pool = pool().await;
    let m = module(&pool).await;
    let company = Uuid::new_v4();
    let employee = Uuid::new_v4();
    let t = token_for(company);
    let (y, mo, date) = prev_month();
    let app = create_guarded_timesheet_routes(&m);

    let (s, entry_id) = create_entry(app.clone(), &t, employee, date).await;
    assert_eq!(s, StatusCode::CREATED, "seed entry");

    let submit = format!(r#"{{"employeeId":"{employee}","year":{y},"month":{mo}}}"#);
    let s = req(app.clone(), "POST", "/timesheets/periods/submit", &t, submit).await;
    assert_eq!(s, StatusCode::CREATED, "submit");

    // Pending: create, update, and delete are all locked.
    let s = req(app.clone(), "POST", "/timesheets/entries", &t, entry_body(employee, date, 18, 19)).await;
    assert_eq!(s, StatusCode::CONFLICT, "create while pending must be 409 period_locked");
    let s = req(app.clone(), "PUT", &format!("/timesheets/entries/{entry_id}"), &t, entry_body(employee, date, 9, 16)).await;
    assert_eq!(s, StatusCode::CONFLICT, "update while pending must be 409 period_locked");
    let s = req(app.clone(), "DELETE", &format!("/timesheets/entries/{entry_id}"), &t, String::new()).await;
    assert_eq!(s, StatusCode::CONFLICT, "delete while pending must be 409 period_locked");

    // Unwired seam (default): no link, manager approves directly — and approved also locks.
    let approve = format!(r#"{{"employeeId":"{employee}","year":{y},"month":{mo}}}"#);
    let s = req(app.clone(), "POST", "/timesheets/periods/approve", &t, approve.clone()).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "direct approve with unwired seam");
    let s = req(app.clone(), "POST", "/timesheets/entries", &t, entry_body(employee, date, 18, 19)).await;
    assert_eq!(s, StatusCode::CONFLICT, "create while approved must be 409 period_locked");

    // Double-approve is a conflict, not a second transition.
    let s = req(app.clone(), "POST", "/timesheets/periods/approve", &t, approve).await;
    assert_eq!(s, StatusCode::CONFLICT, "approve of a non-pending period must be 409 not_pending");

    // Council verdict (chair fix): the lock must also guard the row's SOURCE period —
    // re-dating an approved period's entry into an open month is 409, never a silent
    // move-out that would double-count the hours in two periods.
    let open_month_date = Utc::now().date_naive();
    let s = req(app, "PUT", &format!("/timesheets/entries/{entry_id}"), &t, entry_body(employee, open_month_date, 9, 16)).await;
    assert_eq!(s, StatusCode::CONFLICT, "re-dating an approved period's entry out must be 409 period_locked");

    let y2: i32 = scoped_one(&pool, company, format!(
        "SELECT year FROM timesheet.timesheets WHERE id = '{entry_id}'"
    )).await;
    let m2: i32 = scoped_one(&pool, company, format!(
        "SELECT month FROM timesheet.timesheets WHERE id = '{entry_id}'"
    )).await;
    assert_eq!((y2, m2), (y, mo), "the entry never moved periods");
}

// ─── TS-5: reject reopens the period; re-submit revives the SAME cycle row ─────

#[tokio::test]
async fn guarded_reject_reopens_and_resubmit_revives() {
    let pool = pool().await;
    let m = module(&pool).await;
    let company = Uuid::new_v4();
    let employee = Uuid::new_v4();
    let t = token_for(company);
    let (y, mo, date) = prev_month();
    let app = create_guarded_timesheet_routes(&m);

    create_entry(app.clone(), &t, employee, date).await;
    let submit = format!(r#"{{"employeeId":"{employee}","year":{y},"month":{mo}}}"#);
    let (s, j1) = req_full(app.clone(), "POST", "/timesheets/periods/submit", &t, submit.clone()).await;
    assert_eq!(s, StatusCode::CREATED, "first submit");
    let first_id: Uuid = j1.get("id").unwrap().as_str().unwrap().parse().unwrap();

    let reject = format!(r#"{{"employeeId":"{employee}","year":{y},"month":{mo},"remark":"missing overtime"}}"#);
    let s = req(app.clone(), "POST", "/timesheets/periods/reject", &t, reject).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "reject");

    // Reopened: the employee edits again.
    let s = req(app.clone(), "POST", "/timesheets/entries", &t, entry_body(employee, date, 18, 19)).await;
    assert_eq!(s, StatusCode::CREATED, "create after reject reopens the period");

    // Re-submit revives the rejected row into a new pending cycle (same id, no dup row).
    let (s, j2) = req_full(app.clone(), "POST", "/timesheets/periods/submit", &t, submit).await;
    assert_eq!(s, StatusCode::CREATED, "re-submit");
    let second_id: Uuid = j2.get("id").unwrap().as_str().unwrap().parse().unwrap();
    assert_eq!(first_id, second_id, "revive keeps the same period row id");

    let status: String = scoped_one(&pool, company, format!(
        "SELECT status::text FROM timesheet.timesheet_approvals WHERE employee_id = '{employee}' AND year = {y} AND month = {mo}"
    )).await;
    assert_eq!(status, "pending", "revived row is pending again");
    let rows: i64 = scoped_one(&pool, company, format!(
        "SELECT count(*) FROM timesheet.timesheet_approvals WHERE employee_id = '{employee}' AND year = {y} AND month = {mo}"
    )).await;
    assert_eq!(rows, 1, "one cycle row, not a second");
}

// ─── TS-6: submit gates — empty period, double submit ──────────────────────────

#[tokio::test]
async fn guarded_submit_gates_empty_and_double() {
    let pool = pool().await;
    let m = module(&pool).await;
    let company = Uuid::new_v4();
    let employee = Uuid::new_v4();
    let t = token_for(company);
    let (y, mo, date) = prev_month();
    let app = create_guarded_timesheet_routes(&m);

    // No entries at all: refused.
    let submit = format!(r#"{{"employeeId":"{employee}","year":{y},"month":{mo}}}"#);
    let s = req(app.clone(), "POST", "/timesheets/periods/submit", &t, submit.clone()).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "empty period must be 422 empty_period");

    // One entry: submits once, refuses twice.
    create_entry(app.clone(), &t, employee, date).await;
    let s = req(app.clone(), "POST", "/timesheets/periods/submit", &t, submit.clone()).await;
    assert_eq!(s, StatusCode::CREATED, "first submit");
    let s = req(app, "POST", "/timesheets/periods/submit", &t, submit).await;
    assert_eq!(s, StatusCode::CONFLICT, "double submit must be 409 period_already_submitted");
}

// ─── TS-7: TR2 — a linked period is approved ONLY by the engine ────────────────

#[tokio::test]
async fn guarded_tr2_linked_period_fails_closed_until_engine_grants() {
    let pool = pool().await;
    let m = module(&pool).await;
    m.timesheet_write_service.set_approvals(std::sync::Arc::new(StubApprovals {
        verdict: std::sync::Mutex::new(TimesheetVerdict::Pending),
    }));
    let company = Uuid::new_v4();
    let employee = Uuid::new_v4();
    let t = token_for(company);
    let (y, mo, date) = prev_month();
    let app = create_guarded_timesheet_routes(&m);

    create_entry(app.clone(), &t, employee, date).await; // 09:00–17:00 = 8h
    let submit = format!(r#"{{"employeeId":"{employee}","year":{y},"month":{mo}}}"#);
    let s = req(app.clone(), "POST", "/timesheets/periods/submit", &t, submit).await;
    assert_eq!(s, StatusCode::CREATED, "submit files with the engine");

    // The filing linked the period (approval_request_id stamped).
    let linked: i64 = scoped_one(&pool, company, format!(
        "SELECT count(*) FROM timesheet.timesheet_approvals WHERE employee_id = '{employee}' AND year = {y} AND month = {mo} AND approval_request_id IS NOT NULL"
    )).await;
    assert_eq!(linked, 1, "submit stamped the engine link");

    // Engine says Pending → direct approve fails CLOSED, never bypasses.
    let approve = format!(r#"{{"employeeId":"{employee}","year":{y},"month":{mo}}}"#);
    let s = req(app.clone(), "POST", "/timesheets/periods/approve", &t, approve.clone()).await;
    assert_eq!(s, StatusCode::CONFLICT, "pending verdict must fail closed 409 approval_not_granted");

    let status: String = scoped_one(&pool, company, format!(
        "SELECT status::text FROM timesheet.timesheet_approvals WHERE employee_id = '{employee}' AND year = {y} AND month = {mo}"
    )).await;
    assert_eq!(status, "pending", "failed approve left the period pending");

    // Engine flips to Approved → approve passes and stamps the billable aggregate (8h).
    let port = StubApprovals {
        verdict: std::sync::Mutex::new(TimesheetVerdict::Approved),
    };
    // The port is shared state on the service; swap the verdict by re-wiring a granted port.
    m.timesheet_write_service.set_approvals(std::sync::Arc::new(port));
    let s = req(app.clone(), "POST", "/timesheets/periods/approve", &t, approve).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "granted verdict approves");

    let hours: rust_decimal::Decimal = scoped_one(&pool, company, format!(
        "SELECT billable_time FROM timesheet.timesheet_approvals WHERE employee_id = '{employee}' AND year = {y} AND month = {mo}"
    )).await;
    assert_eq!(hours, rust_decimal::Decimal::from(8), "billable_time stamped as the summed hours");
}

// ─── TS-8: cross-company entry invisible (the fence, live) ─────────────────────

#[tokio::test]
async fn cross_company_entry_invisible() {
    let pool = pool().await;
    let m = module(&pool).await;
    let company_a = Uuid::new_v4();
    let employee = Uuid::new_v4();
    let t_a = token_for(company_a);
    let (_, _, date) = prev_month();
    let app = create_guarded_timesheet_routes(&m);

    let (s, entry_id) = create_entry(app.clone(), &t_a, employee, date).await;
    assert_eq!(s, StatusCode::CREATED, "company A entry");

    // Company B's token mutating A's entry id: the lookup runs under B's RLS scope and finds
    // nothing — 404, never a cross-tenant write.
    let t_b = token_for(Uuid::new_v4());
    let s = req(app.clone(), "PUT", &format!("/timesheets/entries/{entry_id}"), &t_b, entry_body(employee, date, 9, 16)).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "other company's entry must be invisible on update");
    let s = req(app.clone(), "DELETE", &format!("/timesheets/entries/{entry_id}"), &t_b, String::new()).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "other company's entry must be invisible on delete");

    // A still can.
    let s = req(app, "PUT", &format!("/timesheets/entries/{entry_id}"), &t_a, entry_body(employee, date, 9, 16)).await;
    assert_eq!(s, StatusCode::OK, "owning company updates its entry");
}

// ─── TS-9: unauthenticated write → 401 ─────────────────────────────────────────

#[tokio::test]
async fn unauthenticated_write_401() {
    let pool = pool().await;
    let m = module(&pool).await;

    let body = format!(
        r#"{{"employeeId":"{}","date":"2026-07-06"}}"#,
        Uuid::new_v4()
    );
    let s = req(create_guarded_timesheet_routes(&m), "POST", "/timesheets/entries", "not-a-token", body).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "no valid token must be 401");
}

// ─── pure policy units (no DB) ─────────────────────────────────────────────────

#[test]
fn last_day_of_month_math() {
    use backbone_timesheet::last_day_of_month;
    use chrono::NaiveDate;

    assert_eq!(last_day_of_month(2026, 1), Some(NaiveDate::from_ymd_opt(2026, 1, 31).unwrap()));
    assert_eq!(last_day_of_month(2024, 2), Some(NaiveDate::from_ymd_opt(2024, 2, 29).unwrap()), "leap year");
    assert_eq!(last_day_of_month(2026, 2), Some(NaiveDate::from_ymd_opt(2026, 2, 28).unwrap()), "common year");
    assert_eq!(last_day_of_month(2026, 12), Some(NaiveDate::from_ymd_opt(2026, 12, 31).unwrap()), "year boundary");
}
