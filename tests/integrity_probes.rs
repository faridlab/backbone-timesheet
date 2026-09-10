//! Integrity probes — route-level (Wave 1 P2, H-6) + the analytic-line probes. The guarded
//! composition locks generic mutation, the period lock and submit window hold, the EXCLUDE
//! overlap surfaces as 409, and the approvals seam fails closed (TR2). The analytic-line probes
//! pin the plain-amount rules: amounts are plain-stored and reprice ONLY on qualifying writes
//! (never on reads), the rate ladder's stages flow through the port, invoiced rows are
//! write-protected (typed error + DB trigger backstop), and leave rows are minted by the
//! regeneration verb only — never by hand.
//!
//! The module is tenant-agnostic (ADR-0029): it mounts no auth middleware of its own and owns
//! no fence. Each request carries the caller identity the composing service's org-auth stack
//! inserts in production — the [`OrgContext`] extension as a request extension, and the ambient
//! request scope bound via `org_scope::with_org_request_scope` (the harness supplies both, the
//! way a host does). The write path relays that scope onto its transactions; the company-keyed
//! host seams (rate lookup, approvals filing) read the scope's legacy company leg — and fail
//! closed when a request has an identity but NO bound scope (the no_org_scope probe).
//!
//! Assertion reads are plain pool reads: the module test DB runs unfenced (the strip left the
//! enable/force flags for the composing decorator to own; this suite's role sees through them).
//!
//! DB: DATABASE_URL wins, else the module's local test DB (`backbone_timesheet_test` on the
//! metaphora dev postgres). Fresh random org/employee ids per test so parallel runs never
//! collide.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{DateTime, Datelike, Duration, Months, NaiveDate, Utc};
use rust_decimal::Decimal;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use backbone_auth::org::OrgContext;
use backbone_timesheet::org_scope::{OrgScope, with_org_request_scope};
use backbone_timesheet::{
    create_guarded_timesheet_routes, RateLookup, RateSet, RateSourceError,
    TimesheetFiling, TimesheetFilingRequest, TimesheetModule, TimesheetRateSource,
    TimesheetSeamError, TimesheetVerdict, UnwiredRateSource,
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

/// The caller identity a request carries in production (inserted by the composing service's
/// org auth layer). The handlers only require its PRESENCE — the extractor rejects a request
/// without it 401 — and derive nothing tenant-shaped from it.
fn caller(org: Uuid) -> OrgContext {
    OrgContext {
        acting_unit_id: org,
        entitled_units: vec![org],
        legacy_company_id: Some(org),
        user_id: "integrity-probe".to_string(),
    }
}

/// One request through the harness the way a composing service drives it: the ambient request
/// scope bound and the caller identity on the request. Returns status + response body (ids come
/// back in it).
async fn req_full(
    app: axum::Router,
    pool: &PgPool,
    org: Uuid,
    method: &str,
    uri: &str,
    body: String,
) -> (StatusCode, serde_json::Value) {
    let scope = OrgScope::for_company_unit(org);
    with_org_request_scope(pool, scope, async move {
        let mut r = Request::builder().method(method).uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body)).unwrap();
        r.extensions_mut().insert(caller(org));
        let resp = app.oneshot(r).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        // A 5xx body carries the service's own error message — surface it so a probe
        // failure explains itself instead of asserting "500 != 201" blindly.
        if std::env::var("PROBE_LOG_BODIES").is_ok() && !status.is_success() {
            eprintln!("probe got {status}: {json}");
        } else if status.is_server_error() {
            eprintln!("probe got {status}: {json}");
        }
        (status, json)
    })
    .await
    .unwrap()
}

async fn req(
    app: axum::Router,
    pool: &PgPool,
    org: Uuid,
    method: &str,
    uri: &str,
    body: String,
) -> StatusCode {
    req_full(app, pool, org, method, uri, body).await.0
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
/// backstops (a raw duplicate insert raising 23505, the invoiced-row guard trigger raising).
async fn exec(pool: &PgPool, sql: String) -> Result<sqlx::postgres::PgQueryResult, sqlx::Error> {
    sqlx::query(&sql).execute(pool).await
}

/// A decimal out of a JSON value whichever way the serializer spelled it (string or number).
fn dec_from_json(v: &serde_json::Value) -> Decimal {
    if let Some(s) = v.as_str() {
        if let Ok(d) = s.parse::<Decimal>() {
            return d;
        }
    }
    if let Some(f) = v.as_f64() {
        if let Some(d) = Decimal::from_f64_retain(f) {
            return d;
        }
    }
    if let Some(i) = v.as_i64() {
        return Decimal::from(i);
    }
    panic!("probe cannot read a decimal out of {v}");
}

// ─── period fixtures ───────────────────────────────────────────────────────────

/// `(year, month, a safe in-month date)` for the PREVIOUS month — always complete, so the
/// submit window is always open for it (today is past its last day by construction).
fn prev_month() -> (i32, i32, NaiveDate) {
    // Derive the TRUE first of the previous month: subtracting Months from today keeps the
    // day-of-month (Aug 29 -> Jul 29), and "+5 days" would then spill into the current month,
    // landing entries in a different (year, month) than the one the probes submit.
    let today = Utc::now().date_naive();
    let first_of_this_month = NaiveDate::from_ymd_opt(today.year(), today.month(), 1).unwrap();
    let first_of_prev = first_of_this_month.checked_sub_months(Months::new(1)).unwrap();
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

async fn create_entry(
    app: axum::Router,
    pool: &PgPool,
    org: Uuid,
    employee: Uuid,
    date: NaiveDate,
) -> (StatusCode, Uuid) {
    let (s, j) = req_full(app, pool, org, "POST", "/timesheets/entries", entry_body(employee, date, 9, 17)).await;
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
    let (_, _, date) = prev_month();

    let (s, id) = create_entry(create_guarded_timesheet_routes(&m), &pool, company, employee, date).await;
    assert_eq!(s, StatusCode::CREATED, "ranged entry create");

    // end <= start is refused before any SQL runs.
    let bad = format!(
        r#"{{"employeeId":"{employee}","date":"{date}","timeStart":"{}","timeEnd":"{}"}}"#,
        at(date, 17).to_rfc3339(), at(date, 9).to_rfc3339(),
    );
    let s = req(create_guarded_timesheet_routes(&m), &pool, company, "POST", "/timesheets/entries", bad).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "reversed bounds must be 422");

    // Bad entryType vocabulary too.
    let bad_type = format!(
        r#"{{"employeeId":"{employee}","date":"{date}","entryType":"nonsense"}}"#
    );
    let s = req(create_guarded_timesheet_routes(&m), &pool, company, "POST", "/timesheets/entries", bad_type).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "garbage entryType must be 422");

    // And the row is live.
    let n: i64 = one(&pool, format!(
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
    let (_, _, date) = prev_month();
    let app = create_guarded_timesheet_routes(&m);

    let (s, _) = create_entry(app.clone(), &pool, company, employee, date).await;
    assert_eq!(s, StatusCode::CREATED, "first entry 09:00–17:00");

    // 10:00–11:00 sits inside [09:00, 17:00) — the constraint is the arbiter.
    let overlapping = entry_body(employee, date, 10, 11);
    let s = req(app.clone(), &pool, company, "POST", "/timesheets/entries", overlapping).await;
    assert_eq!(s, StatusCode::CONFLICT, "overlapping entry must be 409 entry_overlap");

    // Adjacent is fine; duration-only (no bounds) is exempt by the constraint's predicate.
    let adjacent = entry_body(employee, date, 17, 18);
    let s = req(app.clone(), &pool, company, "POST", "/timesheets/entries", adjacent).await;
    assert_eq!(s, StatusCode::CREATED, "adjacent entry is allowed");

    let draft1 = format!(r#"{{"employeeId":"{employee}","date":"{date}"}}"#);
    let s = req(app.clone(), &pool, company, "POST", "/timesheets/entries", draft1.clone()).await;
    assert_eq!(s, StatusCode::CREATED, "duration-only draft #1");
    let s = req(app, &pool, company, "POST", "/timesheets/entries", draft1).await;
    assert_eq!(s, StatusCode::CREATED, "duration-only draft #2 — no bounds, no clash");
}

// ─── TS-3: the submit validation window ────────────────────────────────────────

#[tokio::test]
async fn guarded_submit_window_gates_current_month() {
    let pool = pool().await;
    let m = module(&pool).await;
    let company = Uuid::new_v4();
    let employee = Uuid::new_v4();
    let app = create_guarded_timesheet_routes(&m);

    // The CURRENT month is still in progress — no submitting it, entries or not.
    let today = Utc::now().date_naive();
    let current = format!(
        r#"{{"employeeId":"{employee}","year":{},"month":{}}}"#,
        today.year(), today.month() as i32
    );
    let s = req(app.clone(), &pool, company, "POST", "/timesheets/periods/submit", current).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "in-progress month must be 422 window_not_open");

    // The previous month is complete: an entry in it submits cleanly.
    let (y, mo, date) = prev_month();
    let (s, _) = create_entry(app.clone(), &pool, company, employee, date).await;
    assert_eq!(s, StatusCode::CREATED, "seed entry");
    let past = format!(r#"{{"employeeId":"{employee}","year":{y},"month":{mo}}}"#);
    let (s, _) = req_full(app, &pool, company, "POST", "/timesheets/periods/submit", past).await;
    assert_eq!(s, StatusCode::CREATED, "complete month submits");
}

// ─── TS-4: the period lock freezes entries while pending AND approved ──────────

#[tokio::test]
async fn guarded_period_lock_freezes_entries() {
    let pool = pool().await;
    let m = module(&pool).await;
    let company = Uuid::new_v4();
    let employee = Uuid::new_v4();
    let (y, mo, date) = prev_month();
    let app = create_guarded_timesheet_routes(&m);

    let (s, entry_id) = create_entry(app.clone(), &pool, company, employee, date).await;
    assert_eq!(s, StatusCode::CREATED, "seed entry");

    let submit = format!(r#"{{"employeeId":"{employee}","year":{y},"month":{mo}}}"#);
    let s = req(app.clone(), &pool, company, "POST", "/timesheets/periods/submit", submit).await;
    assert_eq!(s, StatusCode::CREATED, "submit");

    // Pending: create, update, and delete are all locked.
    let s = req(app.clone(), &pool, company, "POST", "/timesheets/entries", entry_body(employee, date, 18, 19)).await;
    assert_eq!(s, StatusCode::CONFLICT, "create while pending must be 409 period_locked");
    let s = req(app.clone(), &pool, company, "PUT", &format!("/timesheets/entries/{entry_id}"), entry_body(employee, date, 9, 16)).await;
    assert_eq!(s, StatusCode::CONFLICT, "update while pending must be 409 period_locked");
    let s = req(app.clone(), &pool, company, "DELETE", &format!("/timesheets/entries/{entry_id}"), String::new()).await;
    assert_eq!(s, StatusCode::CONFLICT, "delete while pending must be 409 period_locked");

    // Unwired seam (default): no link, manager approves directly — and approved also locks.
    let approve = format!(r#"{{"employeeId":"{employee}","year":{y},"month":{mo}}}"#);
    let s = req(app.clone(), &pool, company, "POST", "/timesheets/periods/approve", approve.clone()).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "direct approve with unwired seam");
    let s = req(app.clone(), &pool, company, "POST", "/timesheets/entries", entry_body(employee, date, 18, 19)).await;
    assert_eq!(s, StatusCode::CONFLICT, "create while approved must be 409 period_locked");

    // Double-approve is a conflict, not a second transition.
    let s = req(app.clone(), &pool, company, "POST", "/timesheets/periods/approve", approve).await;
    assert_eq!(s, StatusCode::CONFLICT, "approve of a non-pending period must be 409 not_pending");

    // Council verdict (chair fix): the lock must also guard the row's SOURCE period —
    // re-dating an approved period's entry into an open month is 409, never a silent
    // move-out that would double-count the hours in two periods.
    let open_month_date = Utc::now().date_naive();
    let s = req(app, &pool, company, "PUT", &format!("/timesheets/entries/{entry_id}"), entry_body(employee, open_month_date, 9, 16)).await;
    assert_eq!(s, StatusCode::CONFLICT, "re-dating an approved period's entry out must be 409 period_locked");

    let y2: i32 = one(&pool, format!(
        "SELECT year FROM timesheet.timesheets WHERE id = '{entry_id}'"
    )).await;
    let m2: i32 = one(&pool, format!(
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
    let (y, mo, date) = prev_month();
    let app = create_guarded_timesheet_routes(&m);

    create_entry(app.clone(), &pool, company, employee, date).await;
    let submit = format!(r#"{{"employeeId":"{employee}","year":{y},"month":{mo}}}"#);
    let (s, j1) = req_full(app.clone(), &pool, company, "POST", "/timesheets/periods/submit", submit.clone()).await;
    assert_eq!(s, StatusCode::CREATED, "first submit");
    let first_id: Uuid = j1.get("id").unwrap().as_str().unwrap().parse().unwrap();

    let reject = format!(r#"{{"employeeId":"{employee}","year":{y},"month":{mo},"remark":"missing overtime"}}"#);
    let s = req(app.clone(), &pool, company, "POST", "/timesheets/periods/reject", reject).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "reject");

    // Reopened: the employee edits again.
    let s = req(app.clone(), &pool, company, "POST", "/timesheets/entries", entry_body(employee, date, 18, 19)).await;
    assert_eq!(s, StatusCode::CREATED, "create after reject reopens the period");

    // Re-submit revives the rejected row into a new pending cycle (same id, no dup row).
    let (s, j2) = req_full(app.clone(), &pool, company, "POST", "/timesheets/periods/submit", submit).await;
    assert_eq!(s, StatusCode::CREATED, "re-submit");
    let second_id: Uuid = j2.get("id").unwrap().as_str().unwrap().parse().unwrap();
    assert_eq!(first_id, second_id, "revive keeps the same period row id");

    let status: String = one(&pool, format!(
        "SELECT status::text FROM timesheet.timesheet_approvals WHERE employee_id = '{employee}' AND year = {y} AND month = {mo}"
    )).await;
    assert_eq!(status, "pending", "revived row is pending again");
    let rows: i64 = one(&pool, format!(
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
    let (y, mo, date) = prev_month();
    let app = create_guarded_timesheet_routes(&m);

    // No entries at all: refused.
    let submit = format!(r#"{{"employeeId":"{employee}","year":{y},"month":{mo}}}"#);
    let s = req(app.clone(), &pool, company, "POST", "/timesheets/periods/submit", submit.clone()).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "empty period must be 422 empty_period");

    // One entry: submits once, refuses twice.
    create_entry(app.clone(), &pool, company, employee, date).await;
    let s = req(app.clone(), &pool, company, "POST", "/timesheets/periods/submit", submit.clone()).await;
    assert_eq!(s, StatusCode::CREATED, "first submit");
    let s = req(app, &pool, company, "POST", "/timesheets/periods/submit", submit).await;
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
    let (y, mo, date) = prev_month();
    let app = create_guarded_timesheet_routes(&m);

    create_entry(app.clone(), &pool, company, employee, date).await; // 09:00–17:00 = 8h
    let submit = format!(r#"{{"employeeId":"{employee}","year":{y},"month":{mo}}}"#);
    let s = req(app.clone(), &pool, company, "POST", "/timesheets/periods/submit", submit).await;
    assert_eq!(s, StatusCode::CREATED, "submit files with the engine");

    // The filing linked the period (approval_request_id stamped).
    let linked: i64 = one(&pool, format!(
        "SELECT count(*) FROM timesheet.timesheet_approvals WHERE employee_id = '{employee}' AND year = {y} AND month = {mo} AND approval_request_id IS NOT NULL"
    )).await;
    assert_eq!(linked, 1, "submit stamped the engine link");

    // Engine says Pending → direct approve fails CLOSED, never bypasses.
    let approve = format!(r#"{{"employeeId":"{employee}","year":{y},"month":{mo}}}"#);
    let s = req(app.clone(), &pool, company, "POST", "/timesheets/periods/approve", approve.clone()).await;
    assert_eq!(s, StatusCode::CONFLICT, "pending verdict must fail closed 409 approval_not_granted");

    let status: String = one(&pool, format!(
        "SELECT status::text FROM timesheet.timesheet_approvals WHERE employee_id = '{employee}' AND year = {y} AND month = {mo}"
    )).await;
    assert_eq!(status, "pending", "failed approve left the period pending");

    // Engine flips to Approved → approve passes and stamps the billable aggregate (8h).
    let port = StubApprovals {
        verdict: std::sync::Mutex::new(TimesheetVerdict::Approved),
    };
    // The port is shared state on the service; swap the verdict by re-wiring a granted port.
    m.timesheet_write_service.set_approvals(std::sync::Arc::new(port));
    let s = req(app.clone(), &pool, company, "POST", "/timesheets/periods/approve", approve).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "granted verdict approves");

    let hours: rust_decimal::Decimal = one(&pool, format!(
        "SELECT billable_time FROM timesheet.timesheet_approvals WHERE employee_id = '{employee}' AND year = {y} AND month = {mo}"
    )).await;
    assert_eq!(hours, rust_decimal::Decimal::from(8), "billable_time stamped as the summed hours");
}

// ─── TS-8: the fail-closed twin — an identified request with NO bound scope ────
//
// The module owns no fence anymore (ADR-0029): cross-tenant invisibility is the composing
// decorator's posture, not this suite's. What the module itself MUST hold is the fail-closed
// twin on its company-keyed host seams: a request that carries an identity but no bound
// ambient scope cannot fall back to a guessed tenant — the rate lookup refuses with the
// typed no_org_scope error.

#[tokio::test]
async fn identified_request_without_scope_fails_closed() {
    let pool = pool().await;
    let m = module(&pool).await;
    let org = Uuid::new_v4();
    let employee = Uuid::new_v4();
    let (_, _, date) = prev_month();
    let app = create_guarded_timesheet_routes(&m);

    // Extension inserted (the extractor passes) but NO ambient request scope bound: a create
    // is a qualifying write, its rate resolution needs the legacy leg, and none is resolvable.
    let mut r = Request::builder().method("POST").uri("/timesheets/entries")
        .header("content-type", "application/json")
        .body(Body::from(entry_body(employee, date, 9, 17))).unwrap();
    r.extensions_mut().insert(caller(org));
    let resp = app.oneshot(r).await.unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR, "no bound scope must fail closed, never guess");
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    assert_eq!(
        json.get("error").and_then(|v| v.as_str()),
        Some("no_org_scope"),
        "the typed no_org_scope error names the missing scope"
    );
}

// ─── TS-9: request without a caller identity → 401 ─────────────────────────────

#[tokio::test]
async fn unauthenticated_write_401() {
    let pool = pool().await;
    let m = module(&pool).await;

    let body = format!(
        r#"{{"employeeId":"{}","date":"2026-07-06"}}"#,
        Uuid::new_v4()
    );
    let r = Request::builder().method("POST").uri("/timesheets/entries")
        .header("content-type", "application/json")
        .body(Body::from(body)).unwrap();
    let resp = create_guarded_timesheet_routes(&m).oneshot(r).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "no OrgContext extension must be 401");
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

// ─── analytic-line rate fakes ──────────────────────────────────────────────────

/// A rate source keyed by activity type id — the port shape a host adapter over
/// `project.activity_types` would present. Unknown/absent activity resolves nothing
/// (rates NULL — a visible absence).
struct ActivityRateFake {
    rates: std::collections::HashMap<Uuid, (Decimal, Decimal)>,
    calls: std::sync::atomic::AtomicUsize,
}

impl ActivityRateFake {
    fn new(pairs: Vec<(Uuid, (Decimal, Decimal))>) -> Self {
        Self { rates: pairs.into_iter().collect(), calls: 0.into() }
    }

    fn calls(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[async_trait::async_trait]
impl TimesheetRateSource for ActivityRateFake {
    async fn resolve_rates(&self, req: &RateLookup) -> Result<RateSet, RateSourceError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let resolved = req.activity_type_id.and_then(|a| self.rates.get(&a)).copied();
        Ok(RateSet {
            billing_rate: resolved.map(|(b, _)| b),
            costing_rate: resolved.map(|(_, c)| c),
        })
    }
}

/// A flat rate source — every lookup resolves the same pair (whatever stage it stands for).
struct FlatRateFake {
    billing: Option<Decimal>,
    costing: Option<Decimal>,
    calls: std::sync::atomic::AtomicUsize,
}

impl FlatRateFake {
    fn new(billing: Option<Decimal>, costing: Option<Decimal>) -> Self {
        Self { billing, costing, calls: 0.into() }
    }

    fn calls(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[async_trait::async_trait]
impl TimesheetRateSource for FlatRateFake {
    async fn resolve_rates(&self, _req: &RateLookup) -> Result<RateSet, RateSourceError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(RateSet { billing_rate: self.billing, costing_rate: self.costing })
    }
}

/// Body for a ranged entry with an activity classification (the rate-determining field).
fn entry_body_with_activity(
    employee: Uuid,
    date: NaiveDate,
    start_h: u32,
    end_h: u32,
    activity: Option<Uuid>,
    extra: &str,
) -> String {
    format!(
        r#"{{"employeeId":"{employee}","date":"{date}","timeStart":"{}","timeEnd":"{}","activityTypeId":{}{}}}"#,
        at(date, start_h).to_rfc3339(),
        at(date, end_h).to_rfc3339(),
        activity.map(|a| format!("\"{a}\"")).unwrap_or_else(|| "null".into()),
        extra,
    )
}

// ─── TS-10: plain amounts — reprice on QUALIFYING writes only ─────────────────

#[tokio::test]
async fn ts_tsm1_reprice_on_qualifying_write_only() {
    let pool = pool().await;
    let m = module(&pool).await;
    let act_a = Uuid::new_v4();
    let act_b = Uuid::new_v4();
    let fake = std::sync::Arc::new(ActivityRateFake::new(vec![
        (act_a, (Decimal::from(150), Decimal::from(75))),
        (act_b, (Decimal::from(200), Decimal::from(90))),
    ]));
    m.timesheet_write_service.set_rate_source(fake.clone());
    let company = Uuid::new_v4();
    let employee = Uuid::new_v4();
    let (y, mo, date) = prev_month();
    let app = create_guarded_timesheet_routes(&m);

    // Create: a qualifying write by definition — 8h x (150 / 75) stamped.
    let (s, j) = req_full(
        app.clone(),
        &pool,
        company,
        "POST",
        "/timesheets/entries",
        entry_body_with_activity(employee, date, 9, 17, Some(act_a), ""),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "ranged entry with activity");
    let id: Uuid = j.get("id").and_then(|v| v.as_str()).unwrap().parse().unwrap();
    let q = |col: &str| format!("SELECT {col} FROM timesheet.timesheets WHERE id = '{id}'");
    assert_eq!(one::<Decimal>(&pool, q("unit_amount")).await, Decimal::from(8));
    assert_eq!(one::<Option<Decimal>>(&pool, q("billing_rate")).await, Some(Decimal::from(150)));
    assert_eq!(one::<Option<Decimal>>(&pool, q("costing_rate")).await, Some(Decimal::from(75)));
    assert_eq!(one::<Decimal>(&pool, q("billable_amount")).await, Decimal::from(1200));
    assert_eq!(one::<Decimal>(&pool, q("costing_amount")).await, Decimal::from(600));

    // Qualifying write (windows shrink → hours change): 4h re-priced on the SAME rates.
    let before = fake.calls();
    let s = req(
        app.clone(),
        &pool,
        company,
        "PUT",
        &format!("/timesheets/entries/{id}"),
        entry_body_with_activity(employee, date, 9, 13, Some(act_a), ""),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "hours-changing update");
    assert!(fake.calls() > before, "a qualifying write re-consults the rate source");
    assert_eq!(one::<Decimal>(&pool, q("unit_amount")).await, Decimal::from(4));
    assert_eq!(one::<Decimal>(&pool, q("billable_amount")).await, Decimal::from(600));
    assert_eq!(one::<Decimal>(&pool, q("costing_amount")).await, Decimal::from(300));

    // Qualifying write (activity reclassification): rates re-resolve through the port.
    let s = req(
        app.clone(),
        &pool,
        company,
        "PUT",
        &format!("/timesheets/entries/{id}"),
        entry_body_with_activity(employee, date, 9, 13, Some(act_b), ""),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "activity-changing update");
    assert_eq!(one::<Option<Decimal>>(&pool, q("billing_rate")).await, Some(Decimal::from(200)));
    assert_eq!(one::<Option<Decimal>>(&pool, q("costing_rate")).await, Some(Decimal::from(90)));
    assert_eq!(one::<Decimal>(&pool, q("billable_amount")).await, Decimal::from(800));
    assert_eq!(one::<Decimal>(&pool, q("costing_amount")).await, Decimal::from(360));

    // NON-qualifying write (remark only, same windows + activity): the snapshot is untouched
    // and the port is not even consulted.
    let before = fake.calls();
    let s = req(
        app.clone(),
        &pool,
        company,
        "PUT",
        &format!("/timesheets/entries/{id}"),
        entry_body_with_activity(employee, date, 9, 13, Some(act_b), ",\"remark\":\"moved note\""),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "remark-only update");
    assert_eq!(fake.calls(), before, "a non-qualifying write must not re-consult the rate source");
    assert_eq!(one::<Option<Decimal>>(&pool, q("billing_rate")).await, Some(Decimal::from(200)));
    assert_eq!(one::<Decimal>(&pool, q("billable_amount")).await, Decimal::from(800));

    // is_billable flip: recomputed from the STORED snapshot, still without re-resolving rates.
    let before = fake.calls();
    let s = req(
        app.clone(),
        &pool,
        company,
        "PUT",
        &format!("/timesheets/entries/{id}"),
        entry_body_with_activity(employee, date, 9, 13, Some(act_b), ",\"isBillable\":false"),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "billability flip");
    assert_eq!(fake.calls(), before, "an is_billable flip re-computes from the stored snapshot, not the port");
    assert_eq!(one::<Option<Decimal>>(&pool, q("billing_rate")).await, Some(Decimal::from(200)));
    assert_eq!(one::<Decimal>(&pool, q("billable_amount")).await, Decimal::ZERO);
    assert_eq!(one::<Decimal>(&pool, q("costing_amount")).await, Decimal::from(360));

    // Flipping it back restores the amount from the same stored rate — no repricing happened.
    let s = req(
        app,
        &pool,
        company,
        "PUT",
        &format!("/timesheets/entries/{id}"),
        entry_body_with_activity(employee, date, 9, 13, Some(act_b), ",\"isBillable\":true"),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(one::<Decimal>(&pool, q("billable_amount")).await, Decimal::from(800));
    let _ = (y, mo);
}

// ─── TS-11: plain amounts — no live repricing on reads ────────────────────────

#[tokio::test]
async fn ts_tsm1_no_live_repricing_read() {
    let pool = pool().await;
    let m = module(&pool).await;
    let act = Uuid::new_v4();
    m.timesheet_write_service.set_rate_source(std::sync::Arc::new(ActivityRateFake::new(vec![
        (act, (Decimal::from(150), Decimal::from(75))),
    ])));
    let company = Uuid::new_v4();
    let employee = Uuid::new_v4();
    let (_, _, date) = prev_month();
    let app = create_guarded_timesheet_routes(&m);

    let (s, j) = req_full(
        app.clone(),
        &pool,
        company,
        "POST",
        "/timesheets/entries",
        entry_body_with_activity(employee, date, 9, 17, Some(act), ""),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let id: Uuid = j.get("id").and_then(|v| v.as_str()).unwrap().parse().unwrap();

    // Swap the rate source for one resolving wildly different rates — an edit of a rate
    // SOURCE must never reprice existing rows on its own.
    let swapped = std::sync::Arc::new(FlatRateFake::new(Some(Decimal::from(999)), Some(Decimal::from(888))));
    m.timesheet_write_service.set_rate_source(swapped.clone());

    // A read returns the STORED snapshot, verbatim.
    let (s, j) = req_full(app.clone(), &pool, company, "GET", &format!("/timesheets/{id}"), String::new()).await;
    assert_eq!(s, StatusCode::OK, "entry reads back");
    let j = j.get("data").cloned().unwrap_or(serde_json::Value::Null); // the read route's envelope
    assert_eq!(j.get("entryType").and_then(|v| v.as_str()), Some("work"));
    assert_eq!(dec_from_json(j.get("billingRate").unwrap()), Decimal::from(150));
    assert_eq!(dec_from_json(j.get("costingRate").unwrap()), Decimal::from(75));
    assert_eq!(dec_from_json(j.get("billableAmount").unwrap()), Decimal::from(1200));
    assert_eq!(dec_from_json(j.get("costingAmount").unwrap()), Decimal::from(600));
    assert_eq!(swapped.calls(), 0, "reads never consult the rate source");

    // A non-qualifying write still keeps the stored snapshot under the swapped source.
    let s = req(
        app,
        &pool,
        company,
        "PUT",
        &format!("/timesheets/entries/{id}"),
        entry_body_with_activity(employee, date, 9, 17, Some(act), ",\"remark\":\"keep me\""),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let rate: Option<Decimal> = one(&pool, format!(
        "SELECT billing_rate FROM timesheet.timesheets WHERE id = '{id}'"
    )).await;
    assert_eq!(rate, Some(Decimal::from(150)), "non-qualifying write keeps the snapshot");
    let amount: Decimal = one(&pool, format!(
        "SELECT billable_amount FROM timesheet.timesheets WHERE id = '{id}'"
    )).await;
    assert_eq!(amount, Decimal::from(1200));
}

// ─── TS-12: the rate ladder's stages flow through the port ────────────────────
//
// The ladder's ORDERING (employee hourly cost > activity-type costing > NULL) is host-adapter
// territory — the module only consumes the resolved RateSet. What the module CAN and does
// prove here: each stage's RateSet lands on the row exactly as resolved (employee-stage cost
// only, activity-stage billing+cost, and nothing resolved = visible NULL rates with 0 amounts).

#[tokio::test]
async fn ts_rate_ladder() {
    let pool = pool().await;
    let m = module(&pool).await;
    let company = Uuid::new_v4();
    let employee = Uuid::new_v4();
    let (_, _, date) = prev_month();
    let app = create_guarded_timesheet_routes(&m);

    let day = date + Duration::days(0);
    let body_for = |emp: Uuid, d: NaiveDate| {
        format!(
            r#"{{"employeeId":"{emp}","date":"{d}","timeStart":"{}","timeEnd":"{}"}}"#,
            at(d, 9).to_rfc3339(),
            at(d, 17).to_rfc3339(),
        )
    };

    // Stage 1 — employee hourly cost resolves the cost side only (the hr_timesheet tail:
    // an employee-cost store carries no billing rate).
    m.timesheet_write_service
        .set_rate_source(std::sync::Arc::new(FlatRateFake::new(None, Some(Decimal::from(100)))));
    let (s, _) = req_full(app.clone(), &pool, company, "POST", "/timesheets/entries", body_for(employee, day)).await;
    assert_eq!(s, StatusCode::CREATED, "employee-stage entry");
    let costing: Decimal = one(&pool, format!(
        "SELECT costing_amount FROM timesheet.timesheets WHERE employee_id = '{employee}' AND date = '{day}'"
    )).await;
    assert_eq!(costing, Decimal::from(800), "8h x employee hourly 100");
    let billing_rate: Option<Decimal> = one(&pool, format!(
        "SELECT billing_rate FROM timesheet.timesheets WHERE employee_id = '{employee}' AND date = '{day}'"
    )).await;
    assert_eq!(billing_rate, None, "employee stage carries no billing rate");
    let billable: Decimal = one(&pool, format!(
        "SELECT billable_amount FROM timesheet.timesheets WHERE employee_id = '{employee}' AND date = '{day}'"
    )).await;
    assert_eq!(billable, Decimal::ZERO);

    // Stage 2 — the activity-type fallback resolves billing + costing both.
    let employee2 = Uuid::new_v4();
    let day2 = date + Duration::days(1);
    m.timesheet_write_service
        .set_rate_source(std::sync::Arc::new(FlatRateFake::new(Some(Decimal::from(150)), Some(Decimal::from(60)))));
    let (s, _) = req_full(app.clone(), &pool, company, "POST", "/timesheets/entries", body_for(employee2, day2)).await;
    assert_eq!(s, StatusCode::CREATED, "activity-stage entry");
    let both: (Option<Decimal>, Decimal, Decimal) = one(&pool, format!(
        "SELECT (billing_rate, billable_amount, costing_amount) FROM timesheet.timesheets WHERE employee_id = '{employee2}' AND date = '{day2}'"
    )).await;
    assert_eq!(both, (Some(Decimal::from(150)), Decimal::from(1200), Decimal::from(480)));

    // Stage 3 — nothing resolves: rates visibly NULL, amounts 0.
    let employee3 = Uuid::new_v4();
    let day3 = date + Duration::days(2);
    m.timesheet_write_service.set_rate_source(std::sync::Arc::new(UnwiredRateSource));
    let (s, _) = req_full(app, &pool, company, "POST", "/timesheets/entries", body_for(employee3, day3)).await;
    assert_eq!(s, StatusCode::CREATED, "unwired entry creates fine (rates NULL)");
    let none: (Option<Decimal>, Option<Decimal>, Decimal, Decimal) = one(&pool, format!(
        "SELECT (billing_rate, costing_rate, billable_amount, costing_amount) FROM timesheet.timesheets WHERE employee_id = '{employee3}' AND date = '{day3}'"
    )).await;
    assert_eq!(none, (None, None, Decimal::ZERO, Decimal::ZERO), "visible absence, never an invented rate");
}

// ─── TS-13: invoiced rows are write-protected (typed error + DB trigger) ──────

#[tokio::test]
async fn ts_invoiced_write_guard() {
    let pool = pool().await;
    let m = module(&pool).await;
    let company = Uuid::new_v4();
    let employee = Uuid::new_v4();
    let (_, _, date) = prev_month();
    let app = create_guarded_timesheet_routes(&m);

    let (s, j) = req_full(
        app.clone(),
        &pool,
        company,
        "POST",
        "/timesheets/entries",
        entry_body_with_activity(employee, date, 9, 17, None, ",\"hours\":8"),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let id: Uuid = j.get("id").and_then(|v| v.as_str()).unwrap().parse().unwrap();

    // The billing exit stamps its one-way link (a cross-module writer, simulated raw).
    exec(
        &pool,
        format!("UPDATE timesheet.timesheets SET invoice_id = '{}' WHERE id = '{id}'", Uuid::new_v4()),
    )
    .await
    .unwrap();

    // Service-level guard: update AND delete refuse with the typed 409.
    let s = req(
        app.clone(),
        &pool,
        company,
        "PUT",
        &format!("/timesheets/entries/{id}"),
        entry_body_with_activity(employee, date, 9, 17, None, ",\"remark\":\"nope\""),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "update of an invoiced row must be 409 invoiced_row_locked");
    let s = req(app.clone(), &pool, company, "DELETE", &format!("/timesheets/entries/{id}"), String::new()).await;
    assert_eq!(s, StatusCode::CONFLICT, "delete of an invoiced row must be 409 invoiced_row_locked");

    // DB-level backstop: a raw pricing write on the invoiced row raises inside Postgres.
    let raw = exec(
        &pool,
        format!("UPDATE timesheet.timesheets SET unit_amount = 1 WHERE id = '{id}'"),
    )
    .await;
    let err = raw.expect_err("the invoiced-row guard trigger must raise");
    let msg = err.as_database_error().map(|d| d.message().to_string()).unwrap_or_default();
    assert!(msg.contains("timesheet_invoiced_row_locked"), "trigger raise, got: {msg}");

    // Clearing the link (the reversal path) stays legal — and re-opens the row for edits.
    exec(&pool, format!("UPDATE timesheet.timesheets SET invoice_id = NULL WHERE id = '{id}'"))
        .await
        .unwrap();
    let s = req(app, &pool, company, "PUT", &format!("/timesheets/entries/{id}"),
        entry_body_with_activity(employee, date, 9, 17, None, ",\"remark\":\"re-opened\"")).await;
    assert_eq!(s, StatusCode::OK, "unbilled row is editable again");
}

// ─── TS-14: timeoff rows — verb-minted only, readable back ────────────────────

#[tokio::test]
async fn ts_entry_type_timeoff_writes() {
    let pool = pool().await;
    let m = module(&pool).await;
    let company = Uuid::new_v4();
    let employee = Uuid::new_v4();
    let (_, _, date) = prev_month();
    let app = create_guarded_timesheet_routes(&m);

    // Hand-writing a timeoff row through the guarded surface refuses — leave rows come
    // exclusively from the regeneration verb off a settled timeoff request.
    let hand = format!(
        r#"{{"employeeId":"{employee}","date":"{date}","entryType":"timeoff"}}"#
    );
    let s = req(app.clone(), &pool, company, "POST", "/timesheets/entries", hand).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "hand-written timeoff must be 422");

    // The verb mints them: one day, 8h of absence.
    let request_id = Uuid::new_v4();
    let inserted = m
        .timesheet_write_service
        .regenerate_leave_rows(&backbone_timesheet::LeaveRowSync {
            employee_id: employee,
            timeoff_request_id: request_id,
            project_id: Uuid::new_v4(),
            task_id: None,
            remark: Some("annual leave".into()),
            entries: vec![backbone_timesheet::LeaveDayEntry { date, hours: Decimal::from(8) }],
        })
        .await
        .expect("leave regeneration");
    assert_eq!(inserted, 1);

    // The row reads back with its shape: timeoff entry, the origin key, plain hours, no rates.
    let id: Uuid = one(&pool, format!(
        "SELECT id FROM timesheet.timesheets WHERE source_timeoff_request_id = '{request_id}' AND (metadata->>'deleted_at') IS NULL"
    )).await;
    let (s, j) = req_full(app, &pool, company, "GET", &format!("/timesheets/{id}"), String::new()).await;
    assert_eq!(s, StatusCode::OK, "leave row reads back");
    let j = j.get("data").cloned().unwrap_or(serde_json::Value::Null); // the read route's envelope
    assert_eq!(j.get("entryType").and_then(|v| v.as_str()), Some("timeoff"));
    assert_eq!(
        j.get("sourceTimeoffRequestId").and_then(|v| v.as_str()),
        Some(request_id.to_string().as_str()),
        "the origin key is part of the read shape"
    );
    assert_eq!(dec_from_json(j.get("unitAmount").unwrap()), Decimal::from(8));
    assert!(j.get("billingRate").map(|v| v.is_null()).unwrap_or(false), "leave rows carry no rates");
    let shape: (String, Option<Decimal>, Decimal) = one(&pool, format!(
        "SELECT (entry_type::text, billing_rate, costing_amount) FROM timesheet.timesheets WHERE id = '{id}'"
    )).await;
    assert_eq!(shape, ("timeoff".into(), None, Decimal::ZERO));
}
