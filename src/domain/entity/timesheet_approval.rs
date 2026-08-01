use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;
use rust_decimal::Decimal;

use super::TimesheetApprovalStatus;
use super::AuditMetadata;

/// Strongly-typed ID for TimesheetApproval
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TimesheetApprovalId(pub Uuid);

impl TimesheetApprovalId {
    pub fn new(id: Uuid) -> Self { Self(id) }
    pub fn generate() -> Self { Self(Uuid::new_v4()) }
    pub fn into_inner(self) -> Uuid { self.0 }
}

impl std::fmt::Display for TimesheetApprovalId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for TimesheetApprovalId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl From<Uuid> for TimesheetApprovalId {
    fn from(id: Uuid) -> Self { Self(id) }
}

impl From<TimesheetApprovalId> for Uuid {
    fn from(id: TimesheetApprovalId) -> Self { id.0 }
}

impl AsRef<Uuid> for TimesheetApprovalId {
    fn as_ref(&self) -> &Uuid { &self.0 }
}

impl std::ops::Deref for TimesheetApprovalId {
    type Target = Uuid;
    fn deref(&self) -> &Self::Target { &self.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct TimesheetApproval {
    pub id: Uuid,
    pub company_id: Uuid,
    pub employee_id: Uuid,
    pub approver_id: Option<Uuid>,
    pub year: i32,
    pub month: i32,
    pub remark: Option<String>,
    pub billable_time: Option<Decimal>,
    pub billable_cost: Option<Decimal>,
    pub status: TimesheetApprovalStatus,
    pub data: Option<serde_json::Value>,
    #[serde(default)]
    #[sqlx(json)]
    pub metadata: AuditMetadata,
}

impl TimesheetApproval {
    /// Create a builder for TimesheetApproval
    pub fn builder() -> TimesheetApprovalBuilder {
        TimesheetApprovalBuilder::default()
    }

    /// Create a new TimesheetApproval with required fields
    pub fn new(company_id: Uuid, employee_id: Uuid, year: i32, month: i32, status: TimesheetApprovalStatus) -> Self {
        Self {
            id: Uuid::new_v4(),
            company_id,
            employee_id,
            approver_id: None,
            year,
            month,
            remark: None,
            billable_time: None,
            billable_cost: None,
            status,
            data: None,
            metadata: AuditMetadata::default(),
        }
    }

    /// Get the entity's unique identifier
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    /// Get a strongly-typed ID for this entity
    pub fn typed_id(&self) -> TimesheetApprovalId {
        TimesheetApprovalId(self.id)
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

    /// Get the current status
    pub fn status(&self) -> &TimesheetApprovalStatus {
        &self.status
    }


    // ==========================================================
    // Fluent Setters (with_* for optional fields)
    // ==========================================================

    /// Set the approver_id field (chainable)
    pub fn with_approver_id(mut self, value: Uuid) -> Self {
        self.approver_id = Some(value);
        self
    }

    /// Set the remark field (chainable)
    pub fn with_remark(mut self, value: String) -> Self {
        self.remark = Some(value);
        self
    }

    /// Set the billable_time field (chainable)
    pub fn with_billable_time(mut self, value: Decimal) -> Self {
        self.billable_time = Some(value);
        self
    }

    /// Set the billable_cost field (chainable)
    pub fn with_billable_cost(mut self, value: Decimal) -> Self {
        self.billable_cost = Some(value);
        self
    }

    /// Set the data field (chainable)
    pub fn with_data(mut self, value: serde_json::Value) -> Self {
        self.data = Some(value);
        self
    }

    // ==========================================================
    // Partial Update
    // ==========================================================

    /// Apply partial updates from a map of field name to JSON value
    pub fn apply_patch(&mut self, fields: std::collections::HashMap<String, serde_json::Value>) {
        for (key, value) in fields {
            match key.as_str() {
                "company_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.company_id = v; }
                }
                "employee_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.employee_id = v; }
                }
                "approver_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.approver_id = v; }
                }
                "year" => {
                    if let Ok(v) = serde_json::from_value(value) { self.year = v; }
                }
                "month" => {
                    if let Ok(v) = serde_json::from_value(value) { self.month = v; }
                }
                "remark" => {
                    if let Ok(v) = serde_json::from_value(value) { self.remark = v; }
                }
                "billable_time" => {
                    if let Ok(v) = serde_json::from_value(value) { self.billable_time = v; }
                }
                "billable_cost" => {
                    if let Ok(v) = serde_json::from_value(value) { self.billable_cost = v; }
                }
                "status" => {
                    if let Ok(v) = serde_json::from_value(value) { self.status = v; }
                }
                "data" => {
                    if let Ok(v) = serde_json::from_value(value) { self.data = v; }
                }
                _ => {} // ignore unknown fields
            }
        }
    }

    // <<< CUSTOM METHODS START >>>
    // <<< CUSTOM METHODS END >>>
}

impl super::Entity for TimesheetApproval {
    type Id = Uuid;

    fn entity_id(&self) -> &Self::Id {
        &self.id
    }

    fn entity_type() -> &'static str {
        "TimesheetApproval"
    }
}

impl backbone_core::PersistentEntity for TimesheetApproval {
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

impl backbone_orm::EntityRepoMeta for TimesheetApproval {
    fn column_types() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("id".to_string(), "uuid".to_string());
        m.insert("company_id".to_string(), "uuid".to_string());
        m.insert("employee_id".to_string(), "uuid".to_string());
        m.insert("approver_id".to_string(), "uuid".to_string());
        m.insert("status".to_string(), "timesheet_approval_status".to_string());
        m
    }
    fn search_fields() -> &'static [&'static str] {
        &[]
    }
    fn company_field() -> Option<&'static str> {
        Some("company_id")
    }
}

/// Builder for TimesheetApproval entity
///
/// Provides a fluent API for constructing TimesheetApproval instances.
/// System fields (id, metadata, timestamps) are auto-initialized.
#[derive(Debug, Clone, Default)]
pub struct TimesheetApprovalBuilder {
    company_id: Option<Uuid>,
    employee_id: Option<Uuid>,
    approver_id: Option<Uuid>,
    year: Option<i32>,
    month: Option<i32>,
    remark: Option<String>,
    billable_time: Option<Decimal>,
    billable_cost: Option<Decimal>,
    status: Option<TimesheetApprovalStatus>,
    data: Option<serde_json::Value>,
}

impl TimesheetApprovalBuilder {
    /// Set the company_id field (required)
    pub fn company_id(mut self, value: Uuid) -> Self {
        self.company_id = Some(value);
        self
    }

    /// Set the employee_id field (required)
    pub fn employee_id(mut self, value: Uuid) -> Self {
        self.employee_id = Some(value);
        self
    }

    /// Set the approver_id field (optional)
    pub fn approver_id(mut self, value: Uuid) -> Self {
        self.approver_id = Some(value);
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

    /// Set the remark field (optional)
    pub fn remark(mut self, value: String) -> Self {
        self.remark = Some(value);
        self
    }

    /// Set the billable_time field (optional)
    pub fn billable_time(mut self, value: Decimal) -> Self {
        self.billable_time = Some(value);
        self
    }

    /// Set the billable_cost field (optional)
    pub fn billable_cost(mut self, value: Decimal) -> Self {
        self.billable_cost = Some(value);
        self
    }

    /// Set the status field (default: `TimesheetApprovalStatus::default()`)
    pub fn status(mut self, value: TimesheetApprovalStatus) -> Self {
        self.status = Some(value);
        self
    }

    /// Set the data field (optional)
    pub fn data(mut self, value: serde_json::Value) -> Self {
        self.data = Some(value);
        self
    }

    /// Build the TimesheetApproval entity
    ///
    /// Returns Err if any required field without a default is missing.
    pub fn build(self) -> Result<TimesheetApproval, String> {
        let company_id = self.company_id.ok_or_else(|| "company_id is required".to_string())?;
        let employee_id = self.employee_id.ok_or_else(|| "employee_id is required".to_string())?;
        let year = self.year.ok_or_else(|| "year is required".to_string())?;
        let month = self.month.ok_or_else(|| "month is required".to_string())?;

        Ok(TimesheetApproval {
            id: Uuid::new_v4(),
            company_id,
            employee_id,
            approver_id: self.approver_id,
            year,
            month,
            remark: self.remark,
            billable_time: self.billable_time,
            billable_cost: self.billable_cost,
            status: self.status.unwrap_or(TimesheetApprovalStatus::default()),
            data: self.data,
            metadata: AuditMetadata::default(),
        })
    }
}
