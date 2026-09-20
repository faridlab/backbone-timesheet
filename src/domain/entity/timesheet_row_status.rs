use serde::{Deserialize, Serialize};
use sqlx::Type;
use std::str::FromStr;
#[cfg(feature = "openapi")]
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "timesheet_row_status", rename_all = "snake_case")]
pub enum TimesheetRowStatus {
    Draft,
    Approved,
    Returned,
}

impl std::fmt::Display for TimesheetRowStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Draft => write!(f, "draft"),
            Self::Approved => write!(f, "approved"),
            Self::Returned => write!(f, "returned"),
        }
    }
}

impl FromStr for TimesheetRowStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "draft" => Ok(Self::Draft),
            "approved" => Ok(Self::Approved),
            "returned" => Ok(Self::Returned),
            _ => Err(format!("Unknown TimesheetRowStatus variant: {}", s)),
        }
    }
}

impl Default for TimesheetRowStatus {
    fn default() -> Self {
        Self::Draft
    }
}
