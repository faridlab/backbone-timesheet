use chrono::{DateTime, Utc, NaiveDate};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;
use rust_decimal::Decimal;

use super::TimesheetType;
use super::AuditMetadata;

/// Strongly-typed ID for Timesheet
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TimesheetId(pub Uuid);

impl TimesheetId {
    pub fn new(id: Uuid) -> Self { Self(id) }
    pub fn generate() -> Self { Self(Uuid::new_v4()) }
    pub fn into_inner(self) -> Uuid { self.0 }
}

impl std::fmt::Display for TimesheetId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for TimesheetId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl From<Uuid> for TimesheetId {
    fn from(id: Uuid) -> Self { Self(id) }
}

impl From<TimesheetId> for Uuid {
    fn from(id: TimesheetId) -> Self { id.0 }
}

impl AsRef<Uuid> for TimesheetId {
    fn as_ref(&self) -> &Uuid { &self.0 }
}

impl std::ops::Deref for TimesheetId {
    type Target = Uuid;
    fn deref(&self) -> &Self::Target { &self.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Timesheet {
    pub id: Uuid,
    pub employee_id: Uuid,
    pub project_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub year: i32,
    pub month: i32,
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
    #[serde(default)]
    #[sqlx(json)]
    pub metadata: AuditMetadata,
}

impl Timesheet {
    /// Create a builder for Timesheet
    pub fn builder() -> TimesheetBuilder {
        <TimesheetBuilder as Default>::default()
    }

    /// Create a new Timesheet with required fields
    pub fn new(employee_id: Uuid, year: i32, month: i32, date: NaiveDate, entry_type: TimesheetType, unit_amount: Decimal, currency: String, is_billable: bool, billable_amount: Decimal, costing_amount: Decimal) -> Self {
        Self {
            id: Uuid::new_v4(),
            employee_id,
            project_id: None,
            task_id: None,
            year,
            month,
            date,
            remark: None,
            time_start: None,
            time_end: None,
            entry_type,
            unit_amount,
            currency,
            activity_type_id: None,
            billing_rate: None,
            costing_rate: None,
            is_billable,
            billable_amount,
            costing_amount,
            invoice_id: None,
            source_timeoff_request_id: None,
            metadata: AuditMetadata::default(),
        }
    }

    /// Get the entity's unique identifier
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    /// Get a strongly-typed ID for this entity
    pub fn typed_id(&self) -> TimesheetId {
        TimesheetId(self.id)
    }

    /// Get when this entity was created
    pub fn created_at(&self) -> Option<&DateTime<Utc>> {
        self.metadata.created_at.as_ref()
    }

    /// Get when this entity was last updated
    pub fn updated_at(&self) -> Option<&DateTime<Utc>> {
        self.metadata.updated_at.as_ref()
    }

    /// Check if this entity is soft deleted
    pub fn is_deleted(&self) -> bool {
        self.metadata.deleted_at.is_some()
    }

    /// Check if this entity is active (not deleted)
    pub fn is_active(&self) -> bool {
        self.metadata.deleted_at.is_none()
    }

    /// Get when this entity was deleted
    pub fn deleted_at(&self) -> Option<&DateTime<Utc>> {
        self.metadata.deleted_at.as_ref()
    }

    /// Get who created this entity
    pub fn created_by(&self) -> Option<&Uuid> {
        self.metadata.created_by.as_ref()
    }

    /// Get who last updated this entity
    pub fn updated_by(&self) -> Option<&Uuid> {
        self.metadata.updated_by.as_ref()
    }

    /// Get who deleted this entity
    pub fn deleted_by(&self) -> Option<&Uuid> {
        self.metadata.deleted_by.as_ref()
    }


    // ==========================================================
    // Fluent Setters (with_* for optional fields)
    // ==========================================================

    /// Set the project_id field (chainable)
    pub fn with_project_id(mut self, value: Uuid) -> Self {
        self.project_id = Some(value);
        self
    }

    /// Set the task_id field (chainable)
    pub fn with_task_id(mut self, value: Uuid) -> Self {
        self.task_id = Some(value);
        self
    }

    /// Set the remark field (chainable)
    pub fn with_remark(mut self, value: String) -> Self {
        self.remark = Some(value);
        self
    }

    /// Set the time_start field (chainable)
    pub fn with_time_start(mut self, value: DateTime<Utc>) -> Self {
        self.time_start = Some(value);
        self
    }

    /// Set the time_end field (chainable)
    pub fn with_time_end(mut self, value: DateTime<Utc>) -> Self {
        self.time_end = Some(value);
        self
    }

    /// Set the activity_type_id field (chainable)
    pub fn with_activity_type_id(mut self, value: Uuid) -> Self {
        self.activity_type_id = Some(value);
        self
    }

    /// Set the billing_rate field (chainable)
    pub fn with_billing_rate(mut self, value: Decimal) -> Self {
        self.billing_rate = Some(value);
        self
    }

    /// Set the costing_rate field (chainable)
    pub fn with_costing_rate(mut self, value: Decimal) -> Self {
        self.costing_rate = Some(value);
        self
    }

    /// Set the invoice_id field (chainable)
    pub fn with_invoice_id(mut self, value: Uuid) -> Self {
        self.invoice_id = Some(value);
        self
    }

    /// Set the source_timeoff_request_id field (chainable)
    pub fn with_source_timeoff_request_id(mut self, value: Uuid) -> Self {
        self.source_timeoff_request_id = Some(value);
        self
    }

    // ==========================================================
    // Partial Update
    // ==========================================================

    /// Apply partial updates from a map of field name to JSON value
    pub fn apply_patch(&mut self, fields: std::collections::HashMap<String, serde_json::Value>) {
        for (key, value) in fields {
            match key.as_str() {
                "employee_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.employee_id = v; }
                }
                "project_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.project_id = v; }
                }
                "task_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.task_id = v; }
                }
                "year" => {
                    if let Ok(v) = serde_json::from_value(value) { self.year = v; }
                }
                "month" => {
                    if let Ok(v) = serde_json::from_value(value) { self.month = v; }
                }
                "date" => {
                    if let Ok(v) = serde_json::from_value(value) { self.date = v; }
                }
                "remark" => {
                    if let Ok(v) = serde_json::from_value(value) { self.remark = v; }
                }
                "time_start" => {
                    if let Ok(v) = serde_json::from_value(value) { self.time_start = v; }
                }
                "time_end" => {
                    if let Ok(v) = serde_json::from_value(value) { self.time_end = v; }
                }
                "entry_type" => {
                    if let Ok(v) = serde_json::from_value(value) { self.entry_type = v; }
                }
                "unit_amount" => {
                    if let Ok(v) = serde_json::from_value(value) { self.unit_amount = v; }
                }
                "currency" => {
                    if let Ok(v) = serde_json::from_value(value) { self.currency = v; }
                }
                "activity_type_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.activity_type_id = v; }
                }
                "billing_rate" => {
                    if let Ok(v) = serde_json::from_value(value) { self.billing_rate = v; }
                }
                "costing_rate" => {
                    if let Ok(v) = serde_json::from_value(value) { self.costing_rate = v; }
                }
                "is_billable" => {
                    if let Ok(v) = serde_json::from_value(value) { self.is_billable = v; }
                }
                "billable_amount" => {
                    if let Ok(v) = serde_json::from_value(value) { self.billable_amount = v; }
                }
                "costing_amount" => {
                    if let Ok(v) = serde_json::from_value(value) { self.costing_amount = v; }
                }
                "invoice_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.invoice_id = v; }
                }
                "source_timeoff_request_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.source_timeoff_request_id = v; }
                }
                _ => {} // ignore unknown fields
            }
        }
    }

    // <<< CUSTOM METHODS START >>>
    // <<< CUSTOM METHODS END >>>
}

impl super::Entity for Timesheet {
    type Id = Uuid;

    fn entity_id(&self) -> &Self::Id {
        &self.id
    }

    fn entity_type() -> &'static str {
        "Timesheet"
    }
}

impl backbone_core::PersistentEntity for Timesheet {
    fn entity_id(&self) -> String {
        self.id.to_string()
    }
    fn set_entity_id(&mut self, id: String) {
        if let Ok(uuid) = uuid::Uuid::parse_str(&id) {
            self.id = uuid;
        }
    }
    fn created_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.metadata.created_at
    }
    fn set_created_at(&mut self, ts: chrono::DateTime<chrono::Utc>) {
        self.metadata.created_at = Some(ts);
    }
    fn updated_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.metadata.updated_at
    }
    fn set_updated_at(&mut self, ts: chrono::DateTime<chrono::Utc>) {
        self.metadata.updated_at = Some(ts);
    }
    fn deleted_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.metadata.deleted_at
    }
    fn set_deleted_at(&mut self, ts: Option<chrono::DateTime<chrono::Utc>>) {
        self.metadata.deleted_at = ts;
    }
}

impl backbone_orm::EntityRepoMeta for Timesheet {
    fn column_types() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("id".to_string(), "uuid".to_string());
        m.insert("employee_id".to_string(), "uuid".to_string());
        m.insert("project_id".to_string(), "uuid".to_string());
        m.insert("task_id".to_string(), "uuid".to_string());
        m.insert("activity_type_id".to_string(), "uuid".to_string());
        m.insert("invoice_id".to_string(), "uuid".to_string());
        m.insert("source_timeoff_request_id".to_string(), "uuid".to_string());
        m.insert("entry_type".to_string(), "timesheet_type".to_string());
        m
    }
    fn search_fields() -> &'static [&'static str] {
        &["currency"]
    }
}

/// Builder for Timesheet entity
///
/// Provides a fluent API for constructing Timesheet instances.
/// System fields (id, metadata, timestamps) are auto-initialized.
#[derive(Debug, Clone, Default)]
pub struct TimesheetBuilder {
    employee_id: Option<Uuid>,
    project_id: Option<Uuid>,
    task_id: Option<Uuid>,
    year: Option<i32>,
    month: Option<i32>,
    date: Option<NaiveDate>,
    remark: Option<String>,
    time_start: Option<DateTime<Utc>>,
    time_end: Option<DateTime<Utc>>,
    entry_type: Option<TimesheetType>,
    unit_amount: Option<Decimal>,
    currency: Option<String>,
    activity_type_id: Option<Uuid>,
    billing_rate: Option<Decimal>,
    costing_rate: Option<Decimal>,
    is_billable: Option<bool>,
    billable_amount: Option<Decimal>,
    costing_amount: Option<Decimal>,
    invoice_id: Option<Uuid>,
    source_timeoff_request_id: Option<Uuid>,
}

impl TimesheetBuilder {
    /// Set the employee_id field (required)
    pub fn employee_id(mut self, value: Uuid) -> Self {
        self.employee_id = Some(value);
        self
    }

    /// Set the project_id field (optional)
    pub fn project_id(mut self, value: Uuid) -> Self {
        self.project_id = Some(value);
        self
    }

    /// Set the task_id field (optional)
    pub fn task_id(mut self, value: Uuid) -> Self {
        self.task_id = Some(value);
        self
    }

    /// Set the year field (required)
    pub fn year(mut self, value: i32) -> Self {
        self.year = Some(value);
        self
    }

    /// Set the month field (required)
    pub fn month(mut self, value: i32) -> Self {
        self.month = Some(value);
        self
    }

    /// Set the date field (required)
    pub fn date(mut self, value: NaiveDate) -> Self {
        self.date = Some(value);
        self
    }

    /// Set the remark field (optional)
    pub fn remark(mut self, value: String) -> Self {
        self.remark = Some(value);
        self
    }

    /// Set the time_start field (optional)
    pub fn time_start(mut self, value: DateTime<Utc>) -> Self {
        self.time_start = Some(value);
        self
    }

    /// Set the time_end field (optional)
    pub fn time_end(mut self, value: DateTime<Utc>) -> Self {
        self.time_end = Some(value);
        self
    }

    /// Set the entry_type field (default: `TimesheetType::default()`)
    pub fn entry_type(mut self, value: TimesheetType) -> Self {
        self.entry_type = Some(value);
        self
    }

    /// Set the unit_amount field (default: `Decimal::from(0)`)
    pub fn unit_amount(mut self, value: Decimal) -> Self {
        self.unit_amount = Some(value);
        self
    }

    /// Set the currency field (default: `"IDR".to_string()`)
    pub fn currency(mut self, value: String) -> Self {
        self.currency = Some(value);
        self
    }

    /// Set the activity_type_id field (optional)
    pub fn activity_type_id(mut self, value: Uuid) -> Self {
        self.activity_type_id = Some(value);
        self
    }

    /// Set the billing_rate field (optional)
    pub fn billing_rate(mut self, value: Decimal) -> Self {
        self.billing_rate = Some(value);
        self
    }

    /// Set the costing_rate field (optional)
    pub fn costing_rate(mut self, value: Decimal) -> Self {
        self.costing_rate = Some(value);
        self
    }

    /// Set the is_billable field (default: `true`)
    pub fn is_billable(mut self, value: bool) -> Self {
        self.is_billable = Some(value);
        self
    }

    /// Set the billable_amount field (default: `Decimal::from(0)`)
    pub fn billable_amount(mut self, value: Decimal) -> Self {
        self.billable_amount = Some(value);
        self
    }

    /// Set the costing_amount field (default: `Decimal::from(0)`)
    pub fn costing_amount(mut self, value: Decimal) -> Self {
        self.costing_amount = Some(value);
        self
    }

    /// Set the invoice_id field (optional)
    pub fn invoice_id(mut self, value: Uuid) -> Self {
        self.invoice_id = Some(value);
        self
    }

    /// Set the source_timeoff_request_id field (optional)
    pub fn source_timeoff_request_id(mut self, value: Uuid) -> Self {
        self.source_timeoff_request_id = Some(value);
        self
    }

    /// Build the Timesheet entity
    ///
    /// Returns Err if any required field without a default is missing.
    pub fn build(self) -> Result<Timesheet, String> {
        let employee_id = self.employee_id.ok_or_else(|| "employee_id is required".to_string())?;
        let year = self.year.ok_or_else(|| "year is required".to_string())?;
        let month = self.month.ok_or_else(|| "month is required".to_string())?;
        let date = self.date.ok_or_else(|| "date is required".to_string())?;

        Ok(Timesheet {
            id: Uuid::new_v4(),
            employee_id,
            project_id: self.project_id,
            task_id: self.task_id,
            year,
            month,
            date,
            remark: self.remark,
            time_start: self.time_start,
            time_end: self.time_end,
            entry_type: self.entry_type.unwrap_or_default(),
            unit_amount: self.unit_amount.unwrap_or(Decimal::from(0)),
            currency: self.currency.unwrap_or("IDR".to_string()),
            activity_type_id: self.activity_type_id,
            billing_rate: self.billing_rate,
            costing_rate: self.costing_rate,
            is_billable: self.is_billable.unwrap_or(true),
            billable_amount: self.billable_amount.unwrap_or(Decimal::from(0)),
            costing_amount: self.costing_amount.unwrap_or(Decimal::from(0)),
            invoice_id: self.invoice_id,
            source_timeoff_request_id: self.source_timeoff_request_id,
            metadata: AuditMetadata::default(),
        })
    }
}
