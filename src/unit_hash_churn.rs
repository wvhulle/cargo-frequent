use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    time::SystemTime,
};

use serde::Serialize;

use crate::{
    fingerprint_unit_records::FingerprintUnitRecord,
    fix_hint_library::{FixHint, hints_for_leaf_difference},
};

const COMPARED_RECENT_UNIT_LIMIT: usize = 6;
const DEPENDENCY_RECURSION_LIMIT: usize = 24;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub enum LeafUnitDifference {
    Features {
        crate_name: String,
        only_older: Vec<String>,
        only_newer: Vec<String>,
    },
    DependencySet {
        crate_name: String,
        only_older: Vec<String>,
        only_newer: Vec<String>,
    },
    Profile {
        crate_name: String,
    },
    Rustflags {
        crate_name: String,
        older_flags: Vec<String>,
        newer_flags: Vec<String>,
    },
    Rustc {
        crate_name: String,
    },
    TargetConfig {
        crate_name: String,
    },
    CargoConfig {
        crate_name: String,
    },
    SourceVersion {
        crate_name: String,
    },
    UnresolvedDependency {
        crate_name: String,
        dependency_name: String,
    },
    LocalInputsOnly {
        crate_name: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct UnitChurnFinding {
    pub profile_directory: String,
    pub crate_name: String,
    pub unit_kind: String,
    pub unit_count: usize,
    pub recent_unit_count: usize,
    pub leaf_differences: Vec<LeafUnitDifference>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LeafCauseSummary {
    pub difference: LeafUnitDifference,
    pub explained_crates: Vec<String>,
    pub fix_hints: Vec<FixHint>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct UnitChurnReport {
    pub leaf_causes: Vec<LeafCauseSummary>,
    pub churning_units: Vec<UnitChurnFinding>,
}

#[derive(PartialEq, Eq, Hash, PartialOrd, Ord)]
struct SiblingUnitKey<'record> {
    profile_directory: &'record str,
    crate_name: &'record str,
    unit_kind: &'record str,
    source_path_hash: u64,
    profile_hash: u64,
    compile_kind_hash: u64,
}

impl<'record> SiblingUnitKey<'record> {
    fn of_record(unit_record: &'record FingerprintUnitRecord) -> Self {
        Self {
            profile_directory: &unit_record.profile_directory,
            crate_name: &unit_record.crate_name,
            unit_kind: &unit_record.unit_kind,
            source_path_hash: unit_record.source_path_hash,
            profile_hash: unit_record.profile_hash,
            compile_kind_hash: unit_record.compile_kind_hash,
        }
    }
}

struct FingerprintHashIndex<'record> {
    records_by_hash: HashMap<(&'record str, u64), &'record FingerprintUnitRecord>,
}

impl<'record> FingerprintHashIndex<'record> {
    fn build(unit_records: &'record [FingerprintUnitRecord]) -> Self {
        Self {
            records_by_hash: unit_records
                .iter()
                .filter_map(|unit_record| {
                    unit_record.own_fingerprint_hash.map(|fingerprint_hash| {
                        (
                            (unit_record.profile_directory.as_str(), fingerprint_hash),
                            unit_record,
                        )
                    })
                })
                .collect(),
        }
    }

    fn resolve(
        &self,
        profile_directory: &str,
        fingerprint_hash: u64,
    ) -> Option<&'record FingerprintUnitRecord> {
        self.records_by_hash
            .get(&(profile_directory, fingerprint_hash))
            .copied()
    }
}

#[must_use]
pub fn detect_unit_hash_churn(
    unit_records: &[FingerprintUnitRecord],
    default_members_select_subset: bool,
    recent_cutoff: SystemTime,
) -> UnitChurnReport {
    let hash_index = FingerprintHashIndex::build(unit_records);
    let mut sibling_groups: BTreeMap<SiblingUnitKey<'_>, Vec<&FingerprintUnitRecord>> =
        BTreeMap::new();
    for unit_record in unit_records
        .iter()
        .filter(|unit_record| !unit_record.runs_build_script())
    {
        sibling_groups
            .entry(SiblingUnitKey::of_record(unit_record))
            .or_default()
            .push(unit_record);
    }

    let mut churning_units: Vec<UnitChurnFinding> = sibling_groups
        .into_iter()
        .filter_map(|(sibling_key, mut sibling_units)| {
            sibling_units.sort_by_key(|unit_record| Reverse(unit_record.modified_time));
            let recent_unit_count = sibling_units
                .iter()
                .filter(|unit_record| unit_record.modified_time >= recent_cutoff)
                .count();
            if recent_unit_count == 0 || sibling_units.len() < 2 {
                return None;
            }
            let leaf_differences = sibling_units
                .iter()
                .take(recent_unit_count.min(COMPARED_RECENT_UNIT_LIMIT))
                .zip(sibling_units.iter().skip(1))
                .flat_map(|(newer_unit, older_unit)| {
                    let mut visited_pairs = HashSet::new();
                    explain_unit_difference(
                        older_unit,
                        newer_unit,
                        &hash_index,
                        0,
                        &mut visited_pairs,
                    )
                })
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            Some(UnitChurnFinding {
                profile_directory: sibling_key.profile_directory.to_string(),
                crate_name: sibling_key.crate_name.to_string(),
                unit_kind: sibling_key.unit_kind.to_string(),
                unit_count: sibling_units.len(),
                recent_unit_count,
                leaf_differences,
            })
        })
        .collect();
    churning_units.sort_by(|left_finding, right_finding| {
        right_finding
            .recent_unit_count
            .cmp(&left_finding.recent_unit_count)
            .then(right_finding.unit_count.cmp(&left_finding.unit_count))
            .then(left_finding.crate_name.cmp(&right_finding.crate_name))
    });

    UnitChurnReport {
        leaf_causes: summarize_leaf_causes(&churning_units, default_members_select_subset),
        churning_units,
    }
}

fn summarize_leaf_causes(
    churning_units: &[UnitChurnFinding],
    default_members_select_subset: bool,
) -> Vec<LeafCauseSummary> {
    let mut crates_by_difference: BTreeMap<&LeafUnitDifference, BTreeSet<&str>> = BTreeMap::new();
    for churn_finding in churning_units {
        for leaf_difference in &churn_finding.leaf_differences {
            crates_by_difference
                .entry(leaf_difference)
                .or_default()
                .insert(&churn_finding.crate_name);
        }
    }
    let mut leaf_causes: Vec<LeafCauseSummary> = crates_by_difference
        .into_iter()
        .map(|(leaf_difference, explained_crates)| LeafCauseSummary {
            fix_hints: hints_for_leaf_difference(leaf_difference, default_members_select_subset),
            difference: leaf_difference.clone(),
            explained_crates: explained_crates.into_iter().map(str::to_string).collect(),
        })
        .collect();
    leaf_causes.sort_by(|left_cause, right_cause| {
        right_cause
            .explained_crates
            .len()
            .cmp(&left_cause.explained_crates.len())
    });
    leaf_causes
}

fn explain_unit_difference(
    older_unit: &FingerprintUnitRecord,
    newer_unit: &FingerprintUnitRecord,
    hash_index: &FingerprintHashIndex<'_>,
    recursion_depth: usize,
    visited_pairs: &mut HashSet<(String, String)>,
) -> Vec<LeafUnitDifference> {
    let crate_name = newer_unit.crate_name.clone();
    if older_unit.source_path_hash != newer_unit.source_path_hash {
        return vec![LeafUnitDifference::SourceVersion { crate_name }];
    }
    let mut leaf_differences = own_field_differences(older_unit, newer_unit);
    if !leaf_differences.is_empty() {
        return leaf_differences;
    }

    let older_dependencies = dependency_hashes_by_name(older_unit);
    let newer_dependencies = dependency_hashes_by_name(newer_unit);
    let only_older: Vec<String> = older_dependencies
        .keys()
        .filter(|dependency_name| !newer_dependencies.contains_key(*dependency_name))
        .map(|dependency_name| (*dependency_name).to_string())
        .collect();
    let only_newer: Vec<String> = newer_dependencies
        .keys()
        .filter(|dependency_name| !older_dependencies.contains_key(*dependency_name))
        .map(|dependency_name| (*dependency_name).to_string())
        .collect();
    if !only_older.is_empty() || !only_newer.is_empty() {
        leaf_differences.push(LeafUnitDifference::DependencySet {
            crate_name: crate_name.clone(),
            only_older,
            only_newer,
        });
    }

    let mut unresolved_dependency_names = Vec::new();
    for (dependency_name, older_hashes) in &older_dependencies {
        let Some(newer_hashes) = newer_dependencies.get(dependency_name) else {
            continue;
        };
        let (older_dependency_units, older_unresolved_count) = resolve_dependency_units(
            hash_index,
            &older_unit.profile_directory,
            older_hashes.difference(newer_hashes),
        );
        let (newer_dependency_units, newer_unresolved_count) = resolve_dependency_units(
            hash_index,
            &newer_unit.profile_directory,
            newer_hashes.difference(older_hashes),
        );
        let changed_unit_pairs =
            pair_changed_dependency_units(&older_dependency_units, &newer_dependency_units);
        if changed_unit_pairs.is_empty() && older_unresolved_count + newer_unresolved_count > 0 {
            unresolved_dependency_names.push(*dependency_name);
        }
        for (older_dependency, newer_dependency) in changed_unit_pairs {
            let visit_key = (
                older_dependency.unit_directory.clone(),
                newer_dependency.unit_directory.clone(),
            );
            if recursion_depth < DEPENDENCY_RECURSION_LIMIT && visited_pairs.insert(visit_key) {
                leaf_differences.extend(explain_unit_difference(
                    older_dependency,
                    newer_dependency,
                    hash_index,
                    recursion_depth + 1,
                    visited_pairs,
                ));
            }
        }
    }

    if leaf_differences.is_empty() {
        leaf_differences.extend(
            unresolved_dependency_names
                .into_iter()
                .map(|dependency_name| LeafUnitDifference::UnresolvedDependency {
                    crate_name: crate_name.clone(),
                    dependency_name: dependency_name.to_string(),
                }),
        );
    }
    if leaf_differences.is_empty() && recursion_depth == 0 {
        leaf_differences.push(LeafUnitDifference::LocalInputsOnly { crate_name });
    }
    leaf_differences
}

fn resolve_dependency_units<'record>(
    hash_index: &FingerprintHashIndex<'record>,
    profile_directory: &str,
    fingerprint_hashes: impl Iterator<Item = &'record u64>,
) -> (Vec<&'record FingerprintUnitRecord>, usize) {
    let mut resolved_units = Vec::new();
    let mut unresolved_count = 0;
    for fingerprint_hash in fingerprint_hashes {
        match hash_index.resolve(profile_directory, *fingerprint_hash) {
            Some(resolved_unit) => resolved_units.push(resolved_unit),
            None => unresolved_count += 1,
        }
    }
    (resolved_units, unresolved_count)
}

fn pair_changed_dependency_units<'record>(
    older_dependency_units: &[&'record FingerprintUnitRecord],
    newer_dependency_units: &[&'record FingerprintUnitRecord],
) -> Vec<(
    &'record FingerprintUnitRecord,
    &'record FingerprintUnitRecord,
)> {
    let same_build_role = |older_dependency: &FingerprintUnitRecord,
                           newer_dependency: &FingerprintUnitRecord| {
        older_dependency.unit_kind == newer_dependency.unit_kind
            && older_dependency.profile_hash == newer_dependency.profile_hash
            && older_dependency.compile_kind_hash == newer_dependency.compile_kind_hash
    };
    older_dependency_units
        .iter()
        .filter_map(|older_dependency| {
            newer_dependency_units
                .iter()
                .find(|newer_dependency| same_build_role(older_dependency, newer_dependency))
                .or_else(|| {
                    newer_dependency_units.iter().find(|newer_dependency| {
                        older_dependency.unit_kind == newer_dependency.unit_kind
                    })
                })
                .map(|newer_dependency| (*older_dependency, *newer_dependency))
        })
        .filter(|(older_dependency, newer_dependency)| {
            older_dependency.unit_directory != newer_dependency.unit_directory
        })
        .collect()
}

fn own_field_differences(
    older_unit: &FingerprintUnitRecord,
    newer_unit: &FingerprintUnitRecord,
) -> Vec<LeafUnitDifference> {
    let crate_name = &newer_unit.crate_name;
    let mut leaf_differences = Vec::new();
    if older_unit.features != newer_unit.features {
        let older_features = older_unit.feature_names();
        let newer_features = newer_unit.feature_names();
        leaf_differences.push(LeafUnitDifference::Features {
            crate_name: crate_name.clone(),
            only_older: older_features
                .iter()
                .filter(|feature_name| !newer_features.contains(feature_name))
                .cloned()
                .collect(),
            only_newer: newer_features
                .iter()
                .filter(|feature_name| !older_features.contains(feature_name))
                .cloned()
                .collect(),
        });
    }
    if older_unit.profile_hash != newer_unit.profile_hash {
        leaf_differences.push(LeafUnitDifference::Profile {
            crate_name: crate_name.clone(),
        });
    }
    if older_unit.rustflags != newer_unit.rustflags {
        leaf_differences.push(LeafUnitDifference::Rustflags {
            crate_name: crate_name.clone(),
            older_flags: older_unit.rustflags.clone(),
            newer_flags: newer_unit.rustflags.clone(),
        });
    }
    if older_unit.rustc_hash != newer_unit.rustc_hash {
        leaf_differences.push(LeafUnitDifference::Rustc {
            crate_name: crate_name.clone(),
        });
    }
    if older_unit.target_hash != newer_unit.target_hash {
        leaf_differences.push(LeafUnitDifference::TargetConfig {
            crate_name: crate_name.clone(),
        });
    }
    if older_unit.cargo_config_hash != newer_unit.cargo_config_hash {
        leaf_differences.push(LeafUnitDifference::CargoConfig {
            crate_name: crate_name.clone(),
        });
    }
    leaf_differences
}

fn dependency_hashes_by_name(unit_record: &FingerprintUnitRecord) -> BTreeMap<&str, BTreeSet<u64>> {
    let mut hashes_by_name: BTreeMap<&str, BTreeSet<u64>> = BTreeMap::new();
    for dependency_edge in &unit_record.dependency_edges {
        hashes_by_name
            .entry(&dependency_edge.dependency_name)
            .or_default()
            .insert(dependency_edge.fingerprint_hash);
    }
    hashes_by_name
}

#[cfg(test)]
mod tests {
    use std::{path::Path, thread, time::Duration as StdDuration};

    use tempfile::TempDir;

    use super::*;
    use crate::fingerprint_unit_records::{
        fingerprint_fixture_writer::{FixtureUnit, write_fixture_unit},
        scan_fingerprint_units,
    };

    fn write_with_pause(target_dir: &Path, fixture_unit: &FixtureUnit<'_>) {
        write_fixture_unit(target_dir, fixture_unit);
        thread::sleep(StdDuration::from_millis(15));
    }

    #[test]
    fn traces_build_script_churn_to_leaf_features() {
        let temporary_target = TempDir::new().unwrap();
        let target_dir = temporary_target.path();
        write_with_pause(
            target_dir,
            &FixtureUnit {
                unit_directory: "syn-1111111111111111",
                unit_kind: "lib-syn",
                own_fingerprint_hash: 11,
                features: &["full"],
                ..FixtureUnit::default()
            },
        );
        write_with_pause(
            target_dir,
            &FixtureUnit {
                unit_directory: "bindgen-1111111111111111",
                unit_kind: "lib-bindgen",
                own_fingerprint_hash: 21,
                dependency_edges: &[("syn", 11)],
                ..FixtureUnit::default()
            },
        );
        write_with_pause(
            target_dir,
            &FixtureUnit {
                unit_directory: "pipewire-sys-1111111111111111",
                unit_kind: "build-script-build-script-build",
                own_fingerprint_hash: 31,
                dependency_edges: &[("bindgen", 21)],
                ..FixtureUnit::default()
            },
        );
        write_with_pause(
            target_dir,
            &FixtureUnit {
                unit_directory: "syn-2222222222222222",
                unit_kind: "lib-syn",
                own_fingerprint_hash: 12,
                features: &["full", "visit"],
                ..FixtureUnit::default()
            },
        );
        write_with_pause(
            target_dir,
            &FixtureUnit {
                unit_directory: "bindgen-2222222222222222",
                unit_kind: "lib-bindgen",
                own_fingerprint_hash: 22,
                dependency_edges: &[("syn", 12)],
                ..FixtureUnit::default()
            },
        );
        write_with_pause(
            target_dir,
            &FixtureUnit {
                unit_directory: "pipewire-sys-2222222222222222",
                unit_kind: "build-script-build-script-build",
                own_fingerprint_hash: 32,
                dependency_edges: &[("bindgen", 22)],
                ..FixtureUnit::default()
            },
        );

        let unit_records = scan_fingerprint_units(target_dir).unwrap();
        let churn_report = detect_unit_hash_churn(&unit_records, true, SystemTime::UNIX_EPOCH);

        let pipewire_finding = churn_report
            .churning_units
            .iter()
            .find(|churn_finding| churn_finding.crate_name == "pipewire-sys")
            .expect("pipewire-sys churns");
        assert_eq!(pipewire_finding.unit_count, 2);
        let expected_leaf = LeafUnitDifference::Features {
            crate_name: "syn".to_string(),
            only_older: vec![],
            only_newer: vec!["visit".to_string()],
        };
        assert_eq!(
            pipewire_finding.leaf_differences,
            vec![expected_leaf.clone()]
        );

        let top_cause = &churn_report.leaf_causes[0];
        assert_eq!(top_cause.difference, expected_leaf);
        assert_eq!(
            top_cause.explained_crates,
            vec!["bindgen", "pipewire-sys", "syn"]
        );
        assert!(!top_cause.fix_hints.is_empty(), "feature hint attached");
    }

    #[test]
    fn reports_dependency_that_cannot_be_resolved() {
        let temporary_target = TempDir::new().unwrap();
        let target_dir = temporary_target.path();
        write_with_pause(
            target_dir,
            &FixtureUnit {
                unit_directory: "libspa-1111111111111111",
                unit_kind: "lib-libspa",
                own_fingerprint_hash: 41,
                dependency_edges: &[("libspa_sys", 900)],
                ..FixtureUnit::default()
            },
        );
        write_with_pause(
            target_dir,
            &FixtureUnit {
                unit_directory: "libspa-2222222222222222",
                unit_kind: "lib-libspa",
                own_fingerprint_hash: 42,
                dependency_edges: &[("libspa_sys", 901)],
                ..FixtureUnit::default()
            },
        );

        let unit_records = scan_fingerprint_units(target_dir).unwrap();
        let churn_report = detect_unit_hash_churn(&unit_records, false, SystemTime::UNIX_EPOCH);
        assert_eq!(
            churn_report.churning_units[0].leaf_differences,
            vec![LeafUnitDifference::UnresolvedDependency {
                crate_name: "libspa".to_string(),
                dependency_name: "libspa_sys".to_string(),
            }]
        );
    }

    #[test]
    fn different_versions_are_not_churn() {
        let temporary_target = TempDir::new().unwrap();
        let target_dir = temporary_target.path();
        write_fixture_unit(
            target_dir,
            &FixtureUnit {
                unit_directory: "syn-1111111111111111",
                unit_kind: "lib-syn",
                source_path_hash: 1,
                ..FixtureUnit::default()
            },
        );
        write_fixture_unit(
            target_dir,
            &FixtureUnit {
                unit_directory: "syn-2222222222222222",
                unit_kind: "lib-syn",
                source_path_hash: 2,
                ..FixtureUnit::default()
            },
        );

        let unit_records = scan_fingerprint_units(target_dir).unwrap();
        assert!(
            detect_unit_hash_churn(&unit_records, false, SystemTime::UNIX_EPOCH)
                .churning_units
                .is_empty(),
            "syn 1 and syn 2 live side by side"
        );
    }
}
