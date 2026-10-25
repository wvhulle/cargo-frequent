pub mod env_source_attribution;
pub mod env_value_change;
pub mod fix_hints;
pub mod live_dirty_reasons;
pub mod recorded_env_drift;
pub mod unit_hash_churn;

use std::fmt::{Display, Formatter, Result as FmtResult};

use serde::Serialize;

use crate::{target_reading::CodeLocation, trigger_detection::fix_hints::FixHint};

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceStep {
    pub step_label: &'static str,
    pub step_text: String,
    pub location: CodeLocation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChainLevel {
    pub level_steps: Vec<SourceStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum TriggerSources {
    Unknown,
    CausalChain { chain_levels: Vec<ChainLevel> },
    IndependentFacts { source_steps: Vec<SourceStep> },
}

impl TriggerSources {
    #[must_use]
    pub fn causal_chain(chain_levels: Vec<ChainLevel>) -> Self {
        let chain_levels: Vec<ChainLevel> = chain_levels
            .into_iter()
            .filter(|chain_level| !chain_level.level_steps.is_empty())
            .collect();
        if chain_levels.is_empty() {
            return Self::Unknown;
        }
        Self::CausalChain { chain_levels }
    }

    #[must_use]
    pub fn independent_facts(source_steps: Vec<SourceStep>) -> Self {
        if source_steps.is_empty() {
            return Self::Unknown;
        }
        Self::IndependentFacts { source_steps }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RebuildTrigger {
    pub trigger_label: String,
    pub trigger_location: CodeLocation,
    pub trigger_change: TriggerChange,
    pub observation_label: &'static str,
    pub trigger_sources: TriggerSources,
    pub observing_crates: Vec<ObservingCrate>,
    pub evidence: String,
    pub recurrence: TriggerRecurrence,
    pub fix_hints: Vec<FixHint>,
}
