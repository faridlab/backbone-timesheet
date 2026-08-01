use serde::{Deserialize, Serialize};
use sqlx::Type;
use std::str::FromStr;
#[cfg(feature = "openapi")]
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "timesheet_type", rename_all = "snake_case")]
pub enum TimesheetType {
    Work,
    Overtime,
}

impl std::fmt::Display for TimesheetType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Work => write!(f, "work"),
            Self::Overtime => write!(f, "overtime"),
        }
    }
}

impl FromStr for TimesheetType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "work" => Ok(Self::Work),
            "overtime" => Ok(Self::Overtime),
            _ => Err(format!("Unknown TimesheetType variant: {}", s)),
        }
    }
}

impl Default for TimesheetType {
    fn default() -> Self {
        Self::Work
    }
}
