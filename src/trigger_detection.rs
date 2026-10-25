pub mod env_value_change;
pub mod fix_hints;
pub mod live_dirty_reasons;
pub mod recorded_env_drift;
pub mod unit_hash_churn;

use serde::Serialize;

use crate::trigger_detection::fix_hints::FixHint;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct ObservingCrate {
    pub crate_name: String,
    pub watches_through_build_script: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RebuildTrigger {
    pub trigger_label: String,
    pub change_summary: String,
    pub observation_label: &'static str,
    pub observing_crates: Vec<ObservingCrate>,
    pub evidence: String,
    pub fix_hints: Vec<FixHint>,
}
