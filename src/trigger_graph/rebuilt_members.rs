use std::{
    collections::{BTreeMap, BTreeSet},
    iter,
};

use serde::Serialize;

use crate::{
    target_reading::fingerprint_units::FingerprintUnit, trigger_detection::RebuildTrigger,
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

pub fn trace_trigger_paths(
    rebuild_triggers: Vec<RebuildTrigger>,
    fingerprint_units: &[FingerprintUnit],
    member_names: &BTreeSet<String>,
) -> Vec<TriggerPath> {
    let dependents_by_dependency = fingerprint_units.iter().fold(
        BTreeMap::new(),
        |mut dependents_by_dependency: BTreeMap<String, BTreeSet<String>>, fingerprint_unit| {
            let dependent_name = fingerprint_unit.crate_name.replace('-', "_");
            fingerprint_unit
                .dependency_edges
                .iter()
                .for_each(|dependency_edge| {
                    dependents_by_dependency
                        .entry(dependency_edge.dependency_name.replace('-', "_"))
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

    let mut trigger_paths: Vec<TriggerPath> = rebuild_triggers
        .into_iter()
        .map(|rebuild_trigger| {
            let observer_names: BTreeSet<String> = rebuild_trigger
                .observing_crates
                .iter()
                .map(|observing_crate| observing_crate.crate_name.replace('-', "_"))
                .collect();
            let mut claimed_members = BTreeSet::new();
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
                        .filter(|member_name| claimed_members.insert((*member_name).clone()))
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
        .collect();
    trigger_paths.truncate(SHOWN_TRIGGER_LIMIT);
    trigger_paths
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
        target_reading::fingerprint_units::{
            fingerprint_fixtures::{FixtureDependency, FixtureUnit, write_fixture_units},
            scan_fingerprint_units,
        },
        trigger_detection::ObservingCrate,
    };

    fn trigger_observed_by(observer_name: &str) -> RebuildTrigger {
        RebuildTrigger {
            trigger_label: "$PROBE".to_string(),
            change_summary: "a -> b".to_string(),
            observation_label: "watched by",
            observing_crates: vec![ObservingCrate {
                crate_name: observer_name.to_string(),
                watches_through_build_script: true,
            }],
            evidence: String::new(),
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

    #[test]
    fn keeps_detector_order_and_limits_trigger_count() {
        let observer_names = ["first", "second", "third", "fourth", "fifth", "sixth"];
        let trigger_paths = trace_trigger_paths(
            observer_names
                .into_iter()
                .map(trigger_observed_by)
                .collect(),
            &[],
            &BTreeSet::new(),
        );
        assert_eq!(
            trigger_paths
                .iter()
                .map(
                    |trigger_path| trigger_path.rebuild_trigger.observing_crates[0]
                        .crate_name
                        .as_str()
                )
                .collect::<Vec<_>>(),
            observer_names[..SHOWN_TRIGGER_LIMIT]
        );
    }
}
