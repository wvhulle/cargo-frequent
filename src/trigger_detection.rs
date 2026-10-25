pub mod env_value_change;
pub mod fix_hints;
pub mod live_dirty_reasons;
pub mod recorded_env_drift;
pub mod unit_hash_churn;

use std::fmt::{Display, Formatter, Result as FmtResult};

use serde::Serialize;

use crate::trigger_detection::fix_hints::FixHint;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct ObservingCrate {
    pub crate_name: String,
    pub watches_through_build_script: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum TriggerRecurrence {
    Recurring,
    OneOff,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum TriggerChange {
    Summary {
        change_text: String,
    },
    Replacement {
        removed_text: String,
        added_text: String,
    },
}

impl Display for TriggerChange {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::Summary { change_text } => write!(formatter, "{change_text}"),
            Self::Replacement {
                removed_text,
                added_text,
            } => write!(formatter, "{removed_text} -> {added_text}"),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RebuildTrigger {
    pub trigger_label: String,
    pub trigger_change: TriggerChange,
    pub observation_label: &'static str,
    pub observing_crates: Vec<ObservingCrate>,
    pub evidence: String,
    pub recurrence: TriggerRecurrence,
    pub fix_hints: Vec<FixHint>,
}
