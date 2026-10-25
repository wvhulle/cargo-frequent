use std::collections::{BTreeMap, BTreeSet};

use crate::{
    target_reading::dirty_reason_parser::{DirtyReason, DirtyUnitEntry},
    trigger_detection::{
        ObservingCrate, RebuildTrigger,
        env_value_change::EnvValueChange,
        fix_hints::{FixHint, env_change_hints},
    },
};

const UNRECOGNIZED_LABEL_LENGTH: usize = 60;

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct LiveTriggerKey {
    trigger_label: String,
    change_summary: String,
    observation_label: &'static str,
}

struct LiveTriggerDescription {
    trigger_key: LiveTriggerKey,
    fix_hints: Vec<FixHint>,
}

struct DescribedDirtyEntry<'entry> {
    trigger_description: LiveTriggerDescription,
    dirty_unit_entry: &'entry DirtyUnitEntry,
}

#[derive(Default)]
struct LiveTriggerGroup {
    observing_crates: BTreeSet<ObservingCrate>,
    fix_hints: BTreeSet<FixHint>,
}

pub fn detect_live_dirty_reasons(dirty_unit_entries: &[DirtyUnitEntry]) -> Vec<RebuildTrigger> {
    dirty_unit_entries
        .iter()
        .filter_map(|dirty_unit_entry| {
            describe_dirty_reason(&dirty_unit_entry.dirty_reason).map(|trigger_description| {
                DescribedDirtyEntry {
                    trigger_description,
                    dirty_unit_entry,
                }
            })
        })
        .fold(
            BTreeMap::new(),
            |mut trigger_groups: BTreeMap<LiveTriggerKey, LiveTriggerGroup>, described_entry| {
                let DescribedDirtyEntry {
                    trigger_description,
                    dirty_unit_entry,
                } = described_entry;
                let trigger_group = trigger_groups
                    .entry(trigger_description.trigger_key)
                    .or_default();
                trigger_group.observing_crates.insert(ObservingCrate {
                    crate_name: dirty_unit_entry.package_name.clone(),
                    watches_through_build_script: dirty_unit_entry.is_build_script,
                });
                trigger_group
                    .fix_hints
                    .extend(trigger_description.fix_hints);
                trigger_groups
            },
        )
        .into_iter()
        .map(|(trigger_key, trigger_group)| RebuildTrigger {
            trigger_label: trigger_key.trigger_label,
            change_summary: trigger_key.change_summary,
            observation_label: trigger_key.observation_label,
            observing_crates: trigger_group.observing_crates.into_iter().collect(),
            evidence: "seen in this build".to_string(),
            fix_hints: trigger_group.fix_hints.into_iter().collect(),
        })
        .collect()
}

fn describe_dirty_reason(dirty_reason: &DirtyReason) -> Option<LiveTriggerDescription> {
    let env_trigger = |variable_name: &str, env_change: EnvValueChange| LiveTriggerDescription {
        fix_hints: env_change_hints(variable_name, &env_change),
        trigger_key: LiveTriggerKey {
            trigger_label: format!("${variable_name}"),
            change_summary: env_change.to_string(),
            observation_label: "watched by",
        },
    };
    let plain_trigger =
        |trigger_label: String, change_summary: String, observation_label: &'static str| {
            LiveTriggerDescription {
                trigger_key: LiveTriggerKey {
                    trigger_label,
                    change_summary,
                    observation_label,
                },
                fix_hints: Vec::new(),
            }
        };
    match dirty_reason {
        DirtyReason::DependencyRebuilt => None,
        DirtyReason::EnvVarSet {
            variable_name,
            current_value,
        } => Some(env_trigger(
            variable_name,
            EnvValueChange::set_to(current_value),
        )),
        DirtyReason::EnvVarUnset {
            variable_name,
            previous_value,
        } => Some(env_trigger(
            variable_name,
            EnvValueChange::unset_from(previous_value),
        )),
        DirtyReason::EnvVarChanged {
            variable_name,
            previous_value,
            current_value,
        } => EnvValueChange::between(previous_value, current_value)
            .map(|env_change| env_trigger(variable_name, env_change)),
        DirtyReason::FileChanged { file_path } => {
            let short_file_path =
                file_path
                    .rsplit('/')
                    .take(2)
                    .fold(String::new(), |short_path, path_segment| {
                        if short_path.is_empty() {
                            path_segment.to_string()
                        } else {
                            format!("{path_segment}/{short_path}")
                        }
                    });
            Some(plain_trigger(
                short_file_path,
                "modified since the last build".to_string(),
                "read by",
            ))
        }
        DirtyReason::RustflagsChanged {
            previous_flags,
            current_flags,
        } => Some(plain_trigger(
            "RUSTFLAGS".to_string(),
            format!(
                "[{}] -> [{}]",
                previous_flags.join(" "),
                current_flags.join(" ")
            ),
            "applies to",
        )),
        DirtyReason::FeaturesChanged {
            previous_features,
            current_features,
        } => Some(plain_trigger(
            "features".to_string(),
            format!("{previous_features} -> {current_features}"),
            "selected for",
        )),
        DirtyReason::ProfileChanged => Some(plain_trigger(
            "profile".to_string(),
            "profile settings changed".to_string(),
            "applies to",
        )),
        DirtyReason::TargetConfigChanged => Some(plain_trigger(
            "target config".to_string(),
            "target settings changed".to_string(),
            "applies to",
        )),
        DirtyReason::Unrecognized { reason_text } => Some(plain_trigger(
            reason_text
                .chars()
                .take(UNRECOGNIZED_LABEL_LENGTH)
                .collect(),
            "reason not recognized by cargo-frequent".to_string(),
            "affects",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirty_entry(
        package_name: &str,
        is_build_script: bool,
        dirty_reason: DirtyReason,
    ) -> DirtyUnitEntry {
        DirtyUnitEntry {
            package_name: package_name.to_string(),
            is_build_script,
            dirty_reason,
        }
    }

    #[test]
    fn groups_units_flagged_by_same_env_change() {
        let changed_path = DirtyReason::EnvVarChanged {
            variable_name: "PATH".to_string(),
            previous_value: "/a".to_string(),
            current_value: "/b".to_string(),
        };
        let rebuild_triggers = detect_live_dirty_reasons(&[
            dirty_entry("pyo3-build-config", true, changed_path.clone()),
            dirty_entry("numpy", true, changed_path),
            dirty_entry("python_utils", false, DirtyReason::DependencyRebuilt),
        ]);

        assert_eq!(rebuild_triggers.len(), 1, "one trigger for both units");
        let path_trigger = &rebuild_triggers[0];
        assert_eq!(path_trigger.trigger_label, "$PATH");
        assert_eq!(path_trigger.change_summary, "/a -> /b");
        assert_eq!(path_trigger.observing_crates.len(), 2);
        assert!(
            path_trigger
                .fix_hints
                .iter()
                .any(|fix_hint| fix_hint.summary.contains("by nature")),
            "volatile hint"
        );
    }

    #[test]
    fn shortens_changed_file_paths() {
        let rebuild_triggers = detect_live_dirty_reasons(&[dirty_entry(
            "probe",
            false,
            DirtyReason::FileChanged {
                file_path: "/tmp/probe/src/lib.rs".to_string(),
            },
        )]);
        assert_eq!(rebuild_triggers[0].trigger_label, "src/lib.rs");
        assert_eq!(rebuild_triggers[0].observation_label, "read by");
    }

    #[test]
    fn dependency_rebuilds_are_not_triggers() {
        assert!(
            detect_live_dirty_reasons(&[dirty_entry(
                "child",
                false,
                DirtyReason::DependencyRebuilt
            )])
            .is_empty(),
            "dependency rebuilds follow from another trigger"
        );
    }
}
