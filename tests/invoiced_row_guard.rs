//! An invoiced analytic line is frozen in the database until its invoice link is cleared, whatever
//! writes it: clearing the link alone is the reversal path, and every other change to an invoiced
//! row refuses — repricing while clearing, moving to another invoice, soft-deleting, deleting.
//! Raw SQL on purpose: this is the floor beneath the service. Requires DATABASE_URL migrated to
//! the module's migrations.

use sqlx::PgPool;
use uuid::Uuid;

async fn pool() -> PgPool {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://postgres:postgres@localhost:5433/backbone_timesheet".to_string());
    PgPool::connect(&url).await.expect("connect DB")
}

/// A billable row of 8 hours already stamped with an invoice.
async fn invoiced_row(pool: &PgPool) -> (Uuid, Uuid) {
    let (id, invoice) = (Uuid::new_v4(), Uuid::new_v4());
    sqlx::query(
        "INSERT INTO timesheet.timesheets (id, employee_id, year, month, date, unit_amount, is_billable, invoice_id) \
         VALUES ($1, $2, 2026, 7, '2026-07-06', 8, true, $3)",
    )
    .bind(id)
    .bind(Uuid::new_v4())
    .bind(invoice)
    .execute(pool)
    .await
    .unwrap();
    (id, invoice)
}

async fn refused(pool: &PgPool, sql: &str, id: Uuid) -> bool {
    match sqlx::query(sql).bind(id).bind(Uuid::new_v4()).execute(pool).await {
        Err(e) => e.to_string().contains("timesheet_invoiced_row_locked"),
        Ok(_) => false,
    }
}

#[tokio::test]
async fn an_invoiced_row_changes_only_by_clearing_its_link() {
    let pool = pool().await;
    let (id, _) = invoiced_row(&pool).await;
    assert!(
        refused(&pool, "UPDATE timesheet.timesheets SET invoice_id = NULL, unit_amount = 9 WHERE id = $1 AND $2::uuid IS NOT NULL", id).await,
        "clearing the link while repricing"
    );
    assert!(refused(&pool, "UPDATE timesheet.timesheets SET invoice_id = $2 WHERE id = $1", id).await, "moving the row to another invoice");
    assert!(
        refused(&pool, "UPDATE timesheet.timesheets SET metadata = metadata || jsonb_build_object('deleted_at', now()) WHERE id = $1 AND $2::uuid IS NOT NULL", id).await,
        "soft-deleting an invoiced row"
    );
    assert!(refused(&pool, "DELETE FROM timesheet.timesheets WHERE id = $1 AND $2::uuid IS NOT NULL", id).await, "deleting an invoiced row");

    let cleared = sqlx::query("UPDATE timesheet.timesheets SET invoice_id = NULL WHERE id = $1").bind(id).execute(&pool).await;
    assert!(cleared.is_ok(), "clearing the link alone is the reversal path: {cleared:?}");
}
