use chrono::{DateTime, Utc, NaiveDate};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;
use rust_decimal::Decimal;
use super::AuditMetadata;

/// Strongly-typed ID for RateCard
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RateCardId(pub Uuid);

impl RateCardId {
    pub fn new(id: Uuid) -> Self { Self(id) }
    pub fn generate() -> Self { Self(Uuid::new_v4()) }
    pub fn into_inner(self) -> Uuid { self.0 }
}

impl std::fmt::Display for RateCardId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for RateCardId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl From<Uuid> for RateCardId {
    fn from(id: Uuid) -> Self { Self(id) }
}

impl From<RateCardId> for Uuid {
    fn from(id: RateCardId) -> Self { id.0 }
}

impl AsRef<Uuid> for RateCardId {
    fn as_ref(&self) -> &Uuid { &self.0 }
}

impl std::ops::Deref for RateCardId {
    type Target = Uuid;
    fn deref(&self) -> &Self::Target { &self.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct RateCard {
    pub id: Uuid,
    pub employee_id: Uuid,
    pub activity_type_id: Option<Uuid>,
    pub billing_rate: Option<Decimal>,
    pub costing_rate: Option<Decimal>,
    pub valid_from: NaiveDate,
    #[serde(default)]
    #[sqlx(json)]
    pub metadata: AuditMetadata,
}

impl RateCard {
    /// Create a builder for RateCard
    pub fn builder() -> RateCardBuilder {
        <RateCardBuilder as Default>::default()
    }

    /// Create a new RateCard with required fields
    pub fn new(employee_id: Uuid, valid_from: NaiveDate) -> Self {
        Self {
            id: Uuid::new_v4(),
            employee_id,
            activity_type_id: None,
            billing_rate: None,
            costing_rate: None,
            valid_from,
            metadata: AuditMetadata::default(),
        }
    }

    /// Get the entity's unique identifier
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    /// Get a strongly-typed ID for this entity
    pub fn typed_id(&self) -> RateCardId {
        RateCardId(self.id)
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
                "activity_type_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.activity_type_id = v; }
                }
                "billing_rate" => {
                    if let Ok(v) = serde_json::from_value(value) { self.billing_rate = v; }
                }
                "costing_rate" => {
                    if let Ok(v) = serde_json::from_value(value) { self.costing_rate = v; }
                }
                "valid_from" => {
                    if let Ok(v) = serde_json::from_value(value) { self.valid_from = v; }
                }
                _ => {} // ignore unknown fields
            }
        }
    }

    // <<< CUSTOM METHODS START >>>
    // <<< CUSTOM METHODS END >>>
}

impl super::Entity for RateCard {
    type Id = Uuid;

    fn entity_id(&self) -> &Self::Id {
        &self.id
    }

    fn entity_type() -> &'static str {
        "RateCard"
    }
}

impl backbone_core::PersistentEntity for RateCard {
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

impl backbone_orm::EntityRepoMeta for RateCard {
    fn column_types() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("id".to_string(), "uuid".to_string());
        m.insert("employee_id".to_string(), "uuid".to_string());
        m.insert("activity_type_id".to_string(), "uuid".to_string());
        m
    }
    fn search_fields() -> &'static [&'static str] {
        &[]
    }
}

/// Builder for RateCard entity
///
/// Provides a fluent API for constructing RateCard instances.
/// System fields (id, metadata, timestamps) are auto-initialized.
#[derive(Debug, Clone, Default)]
pub struct RateCardBuilder {
    employee_id: Option<Uuid>,
    activity_type_id: Option<Uuid>,
    billing_rate: Option<Decimal>,
    costing_rate: Option<Decimal>,
    valid_from: Option<NaiveDate>,
}

impl RateCardBuilder {
    /// Set the employee_id field (required)
    pub fn employee_id(mut self, value: Uuid) -> Self {
        self.employee_id = Some(value);
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

    /// Set the valid_from field (required)
    pub fn valid_from(mut self, value: NaiveDate) -> Self {
        self.valid_from = Some(value);
        self
    }

    /// Build the RateCard entity
    ///
    /// Returns Err if any required field without a default is missing.
    pub fn build(self) -> Result<RateCard, String> {
        let employee_id = self.employee_id.ok_or_else(|| "employee_id is required".to_string())?;
        let valid_from = self.valid_from.ok_or_else(|| "valid_from is required".to_string())?;

        Ok(RateCard {
            id: Uuid::new_v4(),
            employee_id,
            activity_type_id: self.activity_type_id,
            billing_rate: self.billing_rate,
            costing_rate: self.costing_rate,
            valid_from,
            metadata: AuditMetadata::default(),
        })
    }
}
