use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    iter,
};

use serde::Serialize;

use crate::{
    target_reading::fingerprint_units::FingerprintUnit,
    trigger_detection::{RebuildTrigger, TriggerRecurrence},
};

const SHOWN_TRIGGER_LIMIT: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemberRoute {
    pub observing_crate_name: String,
    pub member_name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TriggerPath {
    pub rebuild_trigger: RebuildTrigger,
    pub member_routes: Vec<MemberRoute>,
    pub intermediate_crate_count: usize,
}

#[derive(PartialEq, Eq, Hash)]
struct UnitHashKey<'unit> {
    profile_directory: &'unit str,
    fingerprint_hash: u64,
}

pub fn trace_trigger_paths(
    rebuild_triggers: Vec<RebuildTrigger>,
    fingerprint_units: &[FingerprintUnit],
    member_names: &BTreeSet<String>,
) -> Vec<TriggerPath> {
    let crate_names_by_hash = fingerprint_units.iter().fold(
        HashMap::new(),
        |mut crate_names_by_hash: HashMap<UnitHashKey<'_>, &str>, fingerprint_unit| {
            crate_names_by_hash.insert(
                UnitHashKey {
                    profile_directory: &fingerprint_unit.profile_directory,
                    fingerprint_hash: fingerprint_unit.fingerprint_hash,
                },
                &fingerprint_unit.crate_name,
            );
            crate_names_by_hash
        },
    );
    let dependents_by_dependency = fingerprint_units.iter().fold(
        BTreeMap::new(),
        |mut dependents_by_dependency: BTreeMap<String, BTreeSet<String>>, fingerprint_unit| {
            let dependent_name = fingerprint_unit.crate_name.replace('-', "_");
            fingerprint_unit
                .dependency_edges
                .iter()
                .for_each(|dependency_edge| {
                    let dependency_crate_name = crate_names_by_hash
                        .get(&UnitHashKey {
                            profile_directory: &fingerprint_unit.profile_directory,
                            fingerprint_hash: dependency_edge.fingerprint_hash,
                        })
                        .copied()
                        .unwrap_or(&dependency_edge.dependency_name);
                    dependents_by_dependency
                        .entry(dependency_crate_name.replace('-', "_"))
                        .or_default()
                        .insert(dependent_name.clone());
                });
            dependents_by_dependency
        },
    );
    let members_by_normalized_name = member_names.iter().fold(
        BTreeMap::new(),
        |mut members_by_normalized_name: BTreeMap<String, &String>, member_name| {
            members_by_normalized_name.insert(member_name.replace('-', "_"), member_name);
            members_by_normalized_name
        },
    );

    rebuild_triggers
        .into_iter()
        .map(|rebuild_trigger| {
            let observer_names: BTreeSet<String> = rebuild_trigger
                .observing_crates
                .iter()
                .map(|observing_crate| observing_crate.crate_name.replace('-', "_"))
                .collect();
            let mut intermediate_crates = BTreeSet::new();
            let member_routes = rebuild_trigger
                .observing_crates
                .iter()
                .flat_map(|observing_crate| {
                    let reached_crates = reachable_dependents(
                        &observing_crate.crate_name.replace('-', "_"),
                        &dependents_by_dependency,
                    );
                    reached_crates
                        .iter()
                        .filter(|reached_crate| {
                            !members_by_normalized_name.contains_key(*reached_crate)
                                && !observer_names.contains(*reached_crate)
                        })
                        .for_each(|intermediate_crate| {
                            intermediate_crates.insert(intermediate_crate.clone());
                        });
                    reached_crates
                        .into_iter()
                        .filter_map(|reached_crate| {
                            members_by_normalized_name.get(&reached_crate).copied()
                        })
                        .map(|member_name| MemberRoute {
                            observing_crate_name: observing_crate.crate_name.clone(),
                            member_name: member_name.clone(),
                        })
                        .collect::<Vec<_>>()
                })
                .collect();
            TriggerPath {
                rebuild_trigger,
                member_routes,
                intermediate_crate_count: intermediate_crates.len(),
            }
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OneOffVisibility {
    Hidden,
    Shown,
}

#[derive(Debug, Clone, Serialize)]
pub struct TriggerReport {
    pub trigger_paths: Vec<TriggerPath>,
    pub hidden_trigger_count: usize,
    pub workspace_member_count: usize,
}

pub fn select_trigger_report(
    mut trigger_paths: Vec<TriggerPath>,
    one_off_visibility: OneOffVisibility,
    workspace_member_count: usize,
) -> TriggerReport {
    let total_trigger_count = trigger_paths.len();
    trigger_paths.sort_by_key(|trigger_path| trigger_path.rebuild_trigger.recurrence);
    if one_off_visibility == OneOffVisibility::Hidden {
        trigger_paths.retain(|trigger_path| {
            trigger_path.rebuild_trigger.recurrence == TriggerRecurrence::Recurring
        });
        trigger_paths.truncate(SHOWN_TRIGGER_LIMIT);
    }
    TriggerReport {
        hidden_trigger_count: total_trigger_count - trigger_paths.len(),
        trigger_paths,
        workspace_member_count,
    }
}

fn reachable_dependents(
    starting_crate: &str,
    dependents_by_dependency: &BTreeMap<String, BTreeSet<String>>,
) -> BTreeSet<String> {
    let mut visited_crates = BTreeSet::from([starting_crate.to_string()]);
    iter::successors(Some(vec![starting_crate.to_string()]), |frontier_crates| {
        let next_frontier: Vec<String> = frontier_crates
            .iter()
            .filter_map(|frontier_crate| dependents_by_dependency.get(frontier_crate))
            .flatten()
            .filter(|dependent_crate| visited_crates.insert((*dependent_crate).clone()))
            .cloned()
            .collect();
        (!next_frontier.is_empty()).then_some(next_frontier)
    })
    .flatten()
    .collect()
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::{
        target_reading::{
            CodeLocation,
            fingerprint_units::{
                fingerprint_fixtures::{FixtureDependency, FixtureUnit, write_fixture_units},
                scan_fingerprint_units,
            },
        },
        trigger_detection::{ObservingCrate, TriggerChange, TriggerSources},
    };

    fn trigger_observed_by(observer_name: &str) -> RebuildTrigger {
        RebuildTrigger {
            trigger_location: CodeLocation::Nowhere,
            trigger_label: "$PROBE".to_string(),
            trigger_change: TriggerChange::Summary {
                change_text: "a -> b".to_string(),
            },
            trigger_sources: TriggerSources::Unknown,
            observation_label: "watched by",
            observing_crates: vec![ObservingCrate {
                crate_name: observer_name.to_string(),
                watches_through_build_script: true,
            }],
            evidence: String::new(),
            recurrence: TriggerRecurrence::Recurring,
            fix_hints: Vec::new(),
        }
    }

    #[test]
    fn routes_observer_to_members_through_intermediates() {
        let temporary_target = TempDir::new().unwrap();
        write_fixture_units(
            temporary_target.path(),
            &[
                FixtureUnit {
                    unit_directory: "libspa-1111111111111111",
                    unit_kind: "lib-libspa",
                    dependencies: &[FixtureDependency {
                        dependency_name: "libspa_sys",
                        fingerprint_hash: 1,
                    }],
                    ..FixtureUnit::default()
                },
                FixtureUnit {
                    unit_directory: "sottomano-sources-1111111111111111",
                    unit_kind: "lib-sottomano_sources",
                    dependencies: &[FixtureDependency {
                        dependency_name: "libspa",
                        fingerprint_hash: 2,
                    }],
                    ..FixtureUnit::default()
                },
                FixtureUnit {
                    unit_directory: "sottomano-shell-1111111111111111",
                    unit_kind: "lib-sottomano_shell",
                    dependencies: &[FixtureDependency {
                        dependency_name: "sottomano_sources",
                        fingerprint_hash: 3,
                    }],
                    ..FixtureUnit::default()
                },
            ],
        );
        let member_names = BTreeSet::from([
            "sottomano-sources".to_string(),
            "sottomano-shell".to_string(),
        ]);

        let trigger_paths = trace_trigger_paths(
            vec![trigger_observed_by("libspa-sys")],
            &scan_fingerprint_units(temporary_target.path()).unwrap(),
            &member_names,
        );

        assert_eq!(
            trigger_paths[0].intermediate_crate_count, 1,
            "libspa sits in between"
        );
        assert_eq!(
            trigger_paths[0]
                .member_routes
                .iter()
                .map(|member_route| member_route.member_name.as_str())
                .collect::<Vec<_>>(),
            vec!["sottomano-shell", "sottomano-sources"]
        );
    }

    fn path_with_members(
        observer_name: &str,
        recurrence: TriggerRecurrence,
        member_count: usize,
    ) -> TriggerPath {
        TriggerPath {
            rebuild_trigger: RebuildTrigger {
                recurrence,
                ..trigger_observed_by(observer_name)
            },
            member_routes: (0..member_count)
                .map(|member_position| MemberRoute {
                    observing_crate_name: observer_name.to_string(),
                    member_name: format!("member-{member_position}"),
                })
                .collect(),
            intermediate_crate_count: 0,
        }
    }

    #[test]
    fn follows_renamed_dependencies_through_fingerprint_hashes() {
        let temporary_target = TempDir::new().unwrap();
        write_fixture_units(
            temporary_target.path(),
            &[
                FixtureUnit {
                    unit_directory: "libspa-sys-1111111111111111",
                    unit_kind: "lib-libspa_sys",
                    fingerprint_hash: 7,
                    ..FixtureUnit::default()
                },
                FixtureUnit {
                    unit_directory: "pipewire-1111111111111111",
                    unit_kind: "lib-pipewire",
                    fingerprint_hash: 8,
                    dependencies: &[FixtureDependency {
                        dependency_name: "spa_sys",
                        fingerprint_hash: 7,
                    }],
                    ..FixtureUnit::default()
                },
            ],
        );
        let trigger_paths = trace_trigger_paths(
            vec![trigger_observed_by("libspa-sys")],
            &scan_fingerprint_units(temporary_target.path()).unwrap(),
            &BTreeSet::from(["pipewire".to_string()]),
        );
        assert_eq!(
            trigger_paths[0].member_routes,
            vec![MemberRoute {
                observing_crate_name: "libspa-sys".to_string(),
                member_name: "pipewire".to_string(),
            }],
            "pipewire imports libspa-sys as spa_sys"
        );
    }

    #[test]
    fn keeps_detector_order_and_hides_one_offs() {
        let trigger_report = select_trigger_report(
            vec![
                path_with_members("lockfile", TriggerRecurrence::OneOff, 9),
                path_with_members("narrow", TriggerRecurrence::Recurring, 1),
                path_with_members("wide", TriggerRecurrence::Recurring, 4),
            ],
            OneOffVisibility::Hidden,
            9,
        );
        assert_eq!(
            trigger_report
                .trigger_paths
                .iter()
                .map(
                    |trigger_path| trigger_path.rebuild_trigger.observing_crates[0]
                        .crate_name
                        .as_str()
                )
                .collect::<Vec<_>>(),
            vec!["narrow", "wide"]
        );
        assert_eq!(trigger_report.hidden_trigger_count, 1);
    }

    #[test]
    fn shows_one_offs_on_request() {
        let trigger_report = select_trigger_report(
            vec![path_with_members("lockfile", TriggerRecurrence::OneOff, 9)],
            OneOffVisibility::Shown,
            9,
        );
        assert_eq!(trigger_report.trigger_paths.len(), 1);
        assert_eq!(trigger_report.hidden_trigger_count, 0);
    }
}
