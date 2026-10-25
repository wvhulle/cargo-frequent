use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    time::SystemTime,
};

use serde::Serialize;

use crate::{
    target_reading::fingerprint_units::{FingerprintUnit, UnitRole},
    trigger_detection::{
        ObservingCrate, RebuildTrigger,
        fix_hints::{FixHint, churn_cause_hints},
    },
};

const COMPARED_UNIT_PAIR_LIMIT: usize = 6;
const DEPENDENCY_RECURSION_LIMIT: usize = 24;
const SHOWN_CHURNING_CRATE_LIMIT: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub enum ChurnCause {
    Features {
        crate_name: String,
        removed_features: Vec<String>,
        added_features: Vec<String>,
    },
    DependencySet {
        crate_name: String,
        removed_dependencies: Vec<String>,
        added_dependencies: Vec<String>,
    },
    Profile {
        crate_name: String,
    },
    Rustflags {
        crate_name: String,
        previous_flags: Vec<String>,
        current_flags: Vec<String>,
    },
    Toolchain {
        crate_name: String,
    },
    CargoConfig {
        crate_name: String,
    },
    LockfileVersion {
        crate_name: String,
    },
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct SiblingUnitKey<'unit> {
    profile_directory: &'unit str,
    crate_name: &'unit str,
    unit_kind: &'unit str,
    source_path_hash: u64,
    profile_hash: u64,
    compile_kind_hash: u64,
}

#[derive(PartialEq, Eq, Hash)]
struct HashIndexKey<'unit> {
    profile_directory: &'unit str,
    fingerprint_hash: u64,
}

#[derive(PartialEq, Eq, Hash)]
struct VisitedUnitPair<'unit> {
    older_unit_directory: &'unit str,
    newer_unit_directory: &'unit str,
}

struct UnitPair<'unit> {
    older_unit: &'unit FingerprintUnit,
    newer_unit: &'unit FingerprintUnit,
}

struct ExplainedSiblingGroup<'unit> {
    crate_name: &'unit str,
    churn_causes: BTreeSet<ChurnCause>,
}

type FingerprintHashIndex<'unit> = HashMap<HashIndexKey<'unit>, &'unit FingerprintUnit>;

pub fn detect_unit_hash_churn(
    fingerprint_units: &[FingerprintUnit],
    recent_cutoff: SystemTime,
    default_members_select_subset: bool,
) -> Vec<RebuildTrigger> {
    let hash_index = fingerprint_units.iter().fold(
        FingerprintHashIndex::new(),
        |mut hash_index, fingerprint_unit| {
            hash_index.insert(
                HashIndexKey {
                    profile_directory: &fingerprint_unit.profile_directory,
                    fingerprint_hash: fingerprint_unit.fingerprint_hash,
                },
                fingerprint_unit,
            );
            hash_index
        },
    );
    let sibling_groups = fingerprint_units
        .iter()
        .filter(|fingerprint_unit| fingerprint_unit.unit_role != UnitRole::BuildScriptRun)
        .fold(
            BTreeMap::new(),
            |mut sibling_groups: BTreeMap<SiblingUnitKey<'_>, Vec<&FingerprintUnit>>,
             fingerprint_unit| {
                sibling_groups
                    .entry(SiblingUnitKey {
                        profile_directory: &fingerprint_unit.profile_directory,
                        crate_name: &fingerprint_unit.crate_name,
                        unit_kind: &fingerprint_unit.unit_kind,
                        source_path_hash: fingerprint_unit.source_path_hash,
                        profile_hash: fingerprint_unit.profile_hash,
                        compile_kind_hash: fingerprint_unit.compile_kind_hash,
                    })
                    .or_default()
                    .push(fingerprint_unit);
                sibling_groups
            },
        );

    let churning_crates_by_cause = sibling_groups
        .into_iter()
        .filter_map(|(sibling_key, sibling_units)| {
            explain_sibling_group(
                sibling_key.crate_name,
                sibling_units,
                recent_cutoff,
                &hash_index,
            )
        })
        .fold(
            BTreeMap::new(),
            |mut churning_crates_by_cause: BTreeMap<ChurnCause, BTreeSet<&str>>,
             explained_group| {
                explained_group
                    .churn_causes
                    .into_iter()
                    .for_each(|churn_cause| {
                        churning_crates_by_cause
                            .entry(churn_cause)
                            .or_default()
                            .insert(explained_group.crate_name);
                    });
                churning_crates_by_cause
            },
        );

    merge_churn_triggers(churning_crates_by_cause, default_members_select_subset)
}

fn merge_churn_triggers(
    churning_crates_by_cause: BTreeMap<ChurnCause, BTreeSet<&str>>,
    default_members_select_subset: bool,
) -> Vec<RebuildTrigger> {
    let mut merged_triggers: Vec<MergedChurnTrigger<'_>> = churning_crates_by_cause
        .into_iter()
        .fold(
            BTreeMap::new(),
            |mut merged_by_label: BTreeMap<String, MergedChurnTrigger<'_>>, cause_entry| {
                let (churn_cause, churning_crates) = cause_entry;
                let fix_hints = churn_cause_hints(&churn_cause, default_members_select_subset);
                let described_trigger = describe_churn_cause(churn_cause);
                let merged_trigger = merged_by_label
                    .entry(described_trigger.trigger_label.clone())
                    .or_insert_with(|| MergedChurnTrigger {
                        change_summaries: BTreeSet::new(),
                        churning_crates: BTreeSet::new(),
                        fix_hints: BTreeSet::new(),
                        described_trigger: described_trigger.clone(),
                    });
                merged_trigger
                    .change_summaries
                    .insert(described_trigger.change_summary);
                merged_trigger.churning_crates.extend(churning_crates);
                merged_trigger.fix_hints.extend(fix_hints);
                merged_by_label
            },
        )
        .into_values()
        .collect();
    merged_triggers.sort_by(|left_trigger, right_trigger| {
        right_trigger
            .churning_crates
            .len()
            .cmp(&left_trigger.churning_crates.len())
    });
    merged_triggers
        .into_iter()
        .map(|merged_trigger| RebuildTrigger {
            change_summary: merged_trigger
                .change_summaries
                .into_iter()
                .collect::<Vec<_>>()
                .join(" | "),
            evidence: describe_churning_crates(&merged_trigger.churning_crates),
            fix_hints: merged_trigger.fix_hints.into_iter().collect(),
            ..merged_trigger.described_trigger
        })
        .collect()
}

fn describe_churning_crates(churning_crates: &BTreeSet<&str>) -> String {
    let shown_crates = churning_crates
        .iter()
        .take(SHOWN_CHURNING_CRATE_LIMIT)
        .copied()
        .collect::<Vec<_>>()
        .join(", ");
    let hidden_crate_count = churning_crates
        .len()
        .saturating_sub(SHOWN_CHURNING_CRATE_LIMIT);
    if hidden_crate_count > 0 {
        format!("new unit hashes in the last 24h for {shown_crates} and {hidden_crate_count} more")
    } else {
        format!("new unit hashes in the last 24h for {shown_crates}")
    }
}

struct MergedChurnTrigger<'unit> {
    described_trigger: RebuildTrigger,
    change_summaries: BTreeSet<String>,
    churning_crates: BTreeSet<&'unit str>,
    fix_hints: BTreeSet<FixHint>,
}

fn explain_sibling_group<'unit>(
    crate_name: &'unit str,
    mut sibling_units: Vec<&'unit FingerprintUnit>,
    recent_cutoff: SystemTime,
    hash_index: &FingerprintHashIndex<'unit>,
) -> Option<ExplainedSiblingGroup<'unit>> {
    sibling_units.sort_by_key(|sibling_unit| Reverse(sibling_unit.modified_time));
    let recent_unit_count = sibling_units
        .iter()
        .take_while(|sibling_unit| sibling_unit.modified_time >= recent_cutoff)
        .count();
    if recent_unit_count == 0 || sibling_units.len() < 2 {
        return None;
    }
    let churn_causes = sibling_units
        .windows(2)
        .take(recent_unit_count.min(COMPARED_UNIT_PAIR_LIMIT))
        .flat_map(|adjacent_units| {
            explain_unit_difference(
                &UnitPair {
                    newer_unit: adjacent_units[0],
                    older_unit: adjacent_units[1],
                },
                hash_index,
                0,
                &mut HashSet::new(),
            )
        })
        .collect();
    Some(ExplainedSiblingGroup {
        crate_name,
        churn_causes,
    })
}

fn explain_unit_difference<'unit>(
    unit_pair: &UnitPair<'unit>,
    hash_index: &FingerprintHashIndex<'unit>,
    recursion_depth: usize,
    visited_pairs: &mut HashSet<VisitedUnitPair<'unit>>,
) -> Vec<ChurnCause> {
    let older_unit = unit_pair.older_unit;
    let newer_unit = unit_pair.newer_unit;
    if older_unit.source_path_hash != newer_unit.source_path_hash {
        return vec![ChurnCause::LockfileVersion {
            crate_name: newer_unit.crate_name.clone(),
        }];
    }
    let own_field_causes = own_field_differences(older_unit, newer_unit);
    if !own_field_causes.is_empty() || recursion_depth >= DEPENDENCY_RECURSION_LIMIT {
        return own_field_causes;
    }

    let older_dependencies = dependency_hashes_by_name(older_unit);
    let newer_dependencies = dependency_hashes_by_name(newer_unit);
    let removed_dependencies: Vec<String> = older_dependencies
        .keys()
        .filter(|dependency_name| !newer_dependencies.contains_key(*dependency_name))
        .map(ToString::to_string)
        .collect();
    let added_dependencies: Vec<String> = newer_dependencies
        .keys()
        .filter(|dependency_name| !older_dependencies.contains_key(*dependency_name))
        .map(ToString::to_string)
        .collect();
    let dependency_set_cause = (!removed_dependencies.is_empty() || !added_dependencies.is_empty())
        .then(|| ChurnCause::DependencySet {
            crate_name: newer_unit.crate_name.clone(),
            removed_dependencies,
            added_dependencies,
        });

    let changed_dependency_pairs: Vec<UnitPair<'unit>> = older_dependencies
        .iter()
        .filter_map(|(dependency_name, older_hashes)| {
            let newer_hashes = newer_dependencies.get(dependency_name)?;
            let older_dependency_units = resolve_dependency_units(
                hash_index,
                &older_unit.profile_directory,
                older_hashes.difference(newer_hashes),
            );
            let newer_dependency_units = resolve_dependency_units(
                hash_index,
                &newer_unit.profile_directory,
                newer_hashes.difference(older_hashes),
            );
            Some(pair_changed_dependency_units(
                &older_dependency_units,
                &newer_dependency_units,
            ))
        })
        .flatten()
        .collect();
    let dependency_causes = changed_dependency_pairs.iter().fold(
        Vec::new(),
        |mut dependency_causes, dependency_pair| {
            let first_visit = visited_pairs.insert(VisitedUnitPair {
                older_unit_directory: &dependency_pair.older_unit.unit_directory,
                newer_unit_directory: &dependency_pair.newer_unit.unit_directory,
            });
            if first_visit {
                dependency_causes.extend(explain_unit_difference(
                    dependency_pair,
                    hash_index,
                    recursion_depth + 1,
                    visited_pairs,
                ));
            }
            dependency_causes
        },
    );
    dependency_set_cause
        .into_iter()
        .chain(dependency_causes)
        .collect()
}

fn own_field_differences(
    older_unit: &FingerprintUnit,
    newer_unit: &FingerprintUnit,
) -> Vec<ChurnCause> {
    let crate_name = &newer_unit.crate_name;
    let features_cause =
        (older_unit.feature_names != newer_unit.feature_names).then(|| ChurnCause::Features {
            crate_name: crate_name.clone(),
            removed_features: older_unit
                .feature_names
                .iter()
                .filter(|feature_name| !newer_unit.feature_names.contains(feature_name))
                .cloned()
                .collect(),
            added_features: newer_unit
                .feature_names
                .iter()
                .filter(|feature_name| !older_unit.feature_names.contains(feature_name))
                .cloned()
                .collect(),
        });
    let profile_cause =
        (older_unit.profile_hash != newer_unit.profile_hash).then(|| ChurnCause::Profile {
            crate_name: crate_name.clone(),
        });
    let rustflags_cause =
        (older_unit.rustflags != newer_unit.rustflags).then(|| ChurnCause::Rustflags {
            crate_name: crate_name.clone(),
            previous_flags: older_unit.rustflags.clone(),
            current_flags: newer_unit.rustflags.clone(),
        });
    let toolchain_cause =
        (older_unit.rustc_hash != newer_unit.rustc_hash).then(|| ChurnCause::Toolchain {
            crate_name: crate_name.clone(),
        });
    let config_cause = (older_unit.cargo_config_hash != newer_unit.cargo_config_hash
        || older_unit.target_hash != newer_unit.target_hash)
        .then(|| ChurnCause::CargoConfig {
            crate_name: crate_name.clone(),
        });
    [
        features_cause,
        profile_cause,
        rustflags_cause,
        toolchain_cause,
        config_cause,
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn dependency_hashes_by_name(fingerprint_unit: &FingerprintUnit) -> BTreeMap<&str, BTreeSet<u64>> {
    fingerprint_unit.dependency_edges.iter().fold(
        BTreeMap::new(),
        |mut hashes_by_name: BTreeMap<&str, BTreeSet<u64>>, dependency_edge| {
            hashes_by_name
                .entry(&dependency_edge.dependency_name)
                .or_default()
                .insert(dependency_edge.fingerprint_hash);
            hashes_by_name
        },
    )
}

fn resolve_dependency_units<'unit, 'hashes>(
    hash_index: &FingerprintHashIndex<'unit>,
    profile_directory: &'unit str,
    fingerprint_hashes: impl Iterator<Item = &'hashes u64>,
) -> Vec<&'unit FingerprintUnit> {
    fingerprint_hashes
        .filter_map(|fingerprint_hash| {
            hash_index
                .get(&HashIndexKey {
                    profile_directory,
                    fingerprint_hash: *fingerprint_hash,
                })
                .copied()
        })
        .collect()
}

fn pair_changed_dependency_units<'unit>(
    older_dependency_units: &[&'unit FingerprintUnit],
    newer_dependency_units: &[&'unit FingerprintUnit],
) -> Vec<UnitPair<'unit>> {
    older_dependency_units
        .iter()
        .filter_map(|older_dependency| {
            let same_build_role = newer_dependency_units.iter().find(|newer_dependency| {
                older_dependency.unit_kind == newer_dependency.unit_kind
                    && older_dependency.profile_hash == newer_dependency.profile_hash
                    && older_dependency.compile_kind_hash == newer_dependency.compile_kind_hash
            });
            let same_unit_kind = || {
                newer_dependency_units.iter().find(|newer_dependency| {
                    older_dependency.unit_kind == newer_dependency.unit_kind
                })
            };
            let newer_dependency = same_build_role.or_else(same_unit_kind)?;
            Some(UnitPair {
                older_unit: older_dependency,
                newer_unit: newer_dependency,
            })
        })
        .filter(|dependency_pair| {
            dependency_pair.older_unit.unit_directory != dependency_pair.newer_unit.unit_directory
        })
        .collect()
}

fn describe_churn_cause(churn_cause: ChurnCause) -> RebuildTrigger {
    let churn_trigger =
        |trigger_label: String, change_summary: String, crate_name: String| RebuildTrigger {
            trigger_label,
            change_summary,
            observation_label: "new unit of",
            observing_crates: vec![ObservingCrate {
                crate_name,
                watches_through_build_script: false,
            }],
            evidence: String::new(),
            fix_hints: Vec::new(),
        };
    let describe_added_removed = |added_names: &[String], removed_names: &[String]| {
        added_names
            .iter()
            .map(|added_name| format!("+{added_name}"))
            .chain(
                removed_names
                    .iter()
                    .map(|removed_name| format!("-{removed_name}")),
            )
            .collect::<Vec<_>>()
            .join(" ")
    };
    match churn_cause {
        ChurnCause::Features {
            crate_name,
            removed_features,
            added_features,
        } => churn_trigger(
            format!("{crate_name} features"),
            describe_added_removed(&added_features, &removed_features),
            crate_name,
        ),
        ChurnCause::DependencySet {
            crate_name,
            removed_dependencies,
            added_dependencies,
        } => churn_trigger(
            format!("{crate_name} optional dependencies"),
            describe_added_removed(&added_dependencies, &removed_dependencies),
            crate_name,
        ),
        ChurnCause::Profile { crate_name } => churn_trigger(
            format!("{crate_name} profile"),
            "built with a different profile".to_string(),
            crate_name,
        ),
        ChurnCause::Rustflags {
            crate_name,
            previous_flags,
            current_flags,
        } => churn_trigger(
            format!("{crate_name} RUSTFLAGS"),
            format!(
                "[{}] -> [{}]",
                previous_flags.join(" "),
                current_flags.join(" ")
            ),
            crate_name,
        ),
        ChurnCause::Toolchain { crate_name } => churn_trigger(
            format!("{crate_name} compiler"),
            "built by a different rustc".to_string(),
            crate_name,
        ),
        ChurnCause::CargoConfig { crate_name } => churn_trigger(
            format!("{crate_name} cargo config"),
            "built with different cargo configuration".to_string(),
            crate_name,
        ),
        ChurnCause::LockfileVersion { crate_name } => churn_trigger(
            format!("{crate_name} version"),
            "a different version from Cargo.lock".to_string(),
            crate_name,
        ),
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::target_reading::fingerprint_units::{
        fingerprint_fixtures::{FixtureDependency, FixtureUnit, write_fixture_units},
        scan_fingerprint_units,
    };

    #[test]
    fn traces_build_script_churn_to_leaf_features() {
        let temporary_target = TempDir::new().unwrap();
        write_fixture_units(
            temporary_target.path(),
            &[
                FixtureUnit {
                    unit_directory: "syn-1111111111111111",
                    unit_kind: "lib-syn",
                    fingerprint_hash: 11,
                    feature_names: &["full"],
                    ..FixtureUnit::default()
                },
                FixtureUnit {
                    unit_directory: "bindgen-1111111111111111",
                    unit_kind: "lib-bindgen",
                    fingerprint_hash: 21,
                    dependencies: &[FixtureDependency {
                        dependency_name: "syn",
                        fingerprint_hash: 11,
                    }],
                    ..FixtureUnit::default()
                },
                FixtureUnit {
                    unit_directory: "syn-2222222222222222",
                    unit_kind: "lib-syn",
                    fingerprint_hash: 12,
                    feature_names: &["full", "visit"],
                    ..FixtureUnit::default()
                },
                FixtureUnit {
                    unit_directory: "bindgen-2222222222222222",
                    unit_kind: "lib-bindgen",
                    fingerprint_hash: 22,
                    dependencies: &[FixtureDependency {
                        dependency_name: "syn",
                        fingerprint_hash: 12,
                    }],
                    ..FixtureUnit::default()
                },
            ],
        );

        let rebuild_triggers = detect_unit_hash_churn(
            &scan_fingerprint_units(temporary_target.path()).unwrap(),
            SystemTime::UNIX_EPOCH,
            true,
        );

        assert_eq!(
            rebuild_triggers.len(),
            1,
            "one leaf cause: {rebuild_triggers:?}"
        );
        let syn_trigger = &rebuild_triggers[0];
        assert_eq!(syn_trigger.trigger_label, "syn features");
        assert_eq!(syn_trigger.change_summary, "+visit");
        assert_eq!(syn_trigger.observing_crates[0].crate_name, "syn");
        assert!(
            syn_trigger.evidence.ends_with("for bindgen, syn"),
            "{}",
            syn_trigger.evidence
        );
        assert!(
            syn_trigger.fix_hints[0]
                .suggestion
                .contains("feature-unification"),
            "feature hint attached"
        );
    }

    #[test]
    fn different_versions_side_by_side_are_not_churn() {
        let temporary_target = TempDir::new().unwrap();
        write_fixture_units(
            temporary_target.path(),
            &[
                FixtureUnit {
                    unit_directory: "syn-1111111111111111",
                    unit_kind: "lib-syn",
                    source_path_hash: 1,
                    ..FixtureUnit::default()
                },
                FixtureUnit {
                    unit_directory: "syn-2222222222222222",
                    unit_kind: "lib-syn",
                    source_path_hash: 2,
                    ..FixtureUnit::default()
                },
            ],
        );
        let rebuild_triggers = detect_unit_hash_churn(
            &scan_fingerprint_units(temporary_target.path()).unwrap(),
            SystemTime::UNIX_EPOCH,
            false,
        );
        assert!(rebuild_triggers.is_empty(), "syn 1 and syn 2 coexist");
    }

    #[test]
    fn old_units_outside_window_are_ignored() {
        let temporary_target = TempDir::new().unwrap();
        write_fixture_units(
            temporary_target.path(),
            &[
                FixtureUnit {
                    unit_directory: "syn-1111111111111111",
                    unit_kind: "lib-syn",
                    feature_names: &["full"],
                    ..FixtureUnit::default()
                },
                FixtureUnit {
                    unit_directory: "syn-2222222222222222",
                    unit_kind: "lib-syn",
                    ..FixtureUnit::default()
                },
            ],
        );
        let rebuild_triggers = detect_unit_hash_churn(
            &scan_fingerprint_units(temporary_target.path()).unwrap(),
            SystemTime::now(),
            false,
        );
        assert!(
            rebuild_triggers.is_empty(),
            "nothing built after the cutoff"
        );
    }
}
