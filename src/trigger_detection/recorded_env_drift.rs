use std::{
    collections::{BTreeMap, BTreeSet},
    time::SystemTime,
};

use jiff::{SignedDuration, Unit, fmt::friendly::SpanPrinter};

use crate::{
    target_reading::{
        environment_sources::ProjectEnvironment,
        fingerprint_units::{FingerprintUnit, RecordedEnvValue, UnitRole},
    },
    trigger_detection::{
        ObservingCrate, RebuildTrigger, TriggerRecurrence, TriggerSources,
        env_source_attribution::{EnvChange, PreviousRun, attribute_env_change},
        env_value_change::EnvValueChange,
        fix_hints::{env_change_hints, is_nix_store_update},
    },
};

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct LatestRunKey<'unit> {
    profile_directory: &'unit str,
    crate_name: &'unit str,
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct DriftKey<'unit> {
    variable_name: &'unit str,
    env_change: EnvValueChange,
}

struct DriftOccurrence<'unit> {
    drift_key: DriftKey<'unit>,
    build_script_run: &'unit FingerprintUnit,
}

struct DriftObservers<'unit> {
    crate_names: BTreeSet<&'unit str>,
    newest_recorded_run: SystemTime,
}

pub fn detect_recorded_env_drift(
    fingerprint_units: &[FingerprintUnit],
    project_environment: &ProjectEnvironment,
    lookup_current_env: impl Fn(&str) -> Option<String>,
) -> Vec<RebuildTrigger> {
    let latest_build_script_runs = fingerprint_units
        .iter()
        .filter(|fingerprint_unit| fingerprint_unit.unit_role == UnitRole::BuildScriptRun)
        .fold(
            BTreeMap::new(),
            |mut latest_runs: BTreeMap<LatestRunKey<'_>, &FingerprintUnit>, build_script_run| {
                latest_runs
                    .entry(LatestRunKey {
                        profile_directory: &build_script_run.profile_directory,
                        crate_name: &build_script_run.crate_name,
                    })
                    .and_modify(|latest_run| {
                        if build_script_run.modified_time > latest_run.modified_time {
                            *latest_run = build_script_run;
                        }
                    })
                    .or_insert(build_script_run);
                latest_runs
            },
        );

    let drift_groups = latest_build_script_runs
        .values()
        .flat_map(|build_script_run| {
            build_script_run
                .recorded_env_values
                .iter()
                .filter_map(|recorded_env_value| {
                    drift_against_current_env(recorded_env_value, &lookup_current_env)
                })
                .map(move |drift_key| DriftOccurrence {
                    drift_key,
                    build_script_run,
                })
        })
        .fold(
            BTreeMap::new(),
            |mut drift_groups: BTreeMap<DriftKey<'_>, DriftObservers<'_>>, drift_occurrence| {
                let build_script_run = drift_occurrence.build_script_run;
                let drift_observers = drift_groups
                    .entry(drift_occurrence.drift_key)
                    .or_insert_with(|| DriftObservers {
                        crate_names: BTreeSet::new(),
                        newest_recorded_run: SystemTime::UNIX_EPOCH,
                    });
                drift_observers
                    .crate_names
                    .insert(&build_script_run.crate_name);
                drift_observers.newest_recorded_run = drift_observers
                    .newest_recorded_run
                    .max(build_script_run.modified_time);
                drift_groups
            },
        );

    drift_groups
        .into_iter()
        .map(|(drift_key, drift_observers)| {
            let env_attribution = attribute_env_change(
                &EnvChange {
                    variable_name: drift_key.variable_name,
                    value_change: &drift_key.env_change,
                    previous_run: PreviousRun::RecordedAt(drift_observers.newest_recorded_run),
                },
                project_environment,
            );
            RebuildTrigger {
                trigger_location: env_attribution.cause_location,
                trigger_label: format!("${}", drift_key.variable_name),
                trigger_change: drift_key.env_change.to_trigger_change(),
                trigger_sources: TriggerSources::causal_chain(env_attribution.chain_levels),
                observation_label: "watched by",
                observing_crates: drift_observers
                    .crate_names
                    .into_iter()
                    .map(|crate_name| ObservingCrate {
                        crate_name: crate_name.to_string(),
                        watches_through_build_script: true,
                    })
                    .collect(),
                evidence: format!(
                    "recorded {} ago, reruns on the next build",
                    describe_elapsed_time(drift_observers.newest_recorded_run)
                ),
                recurrence: if is_nix_store_update(&drift_key.env_change) {
                    TriggerRecurrence::OneOff
                } else {
                    TriggerRecurrence::Recurring
                },
                fix_hints: if env_attribution.fix_hints.is_empty() {
                    env_change_hints(drift_key.variable_name, &drift_key.env_change)
                } else {
                    env_attribution.fix_hints
                },
            }
        })
        .collect()
}

fn drift_against_current_env<'unit>(
    recorded_env_value: &'unit RecordedEnvValue,
    lookup_current_env: &impl Fn(&str) -> Option<String>,
) -> Option<DriftKey<'unit>> {
    match recorded_env_value {
        RecordedEnvValue::Set {
            variable_name,
            value,
        } => {
            let Some(current_value) = lookup_current_env(variable_name) else {
                return Some(DriftKey {
                    variable_name,
                    env_change: EnvValueChange::unset_from(value),
                });
            };
            Some(DriftKey {
                variable_name,
                env_change: EnvValueChange::between(value, &current_value)?,
            })
        }
        RecordedEnvValue::Unset { variable_name } => Some(DriftKey {
            variable_name,
            env_change: EnvValueChange::set_to(&lookup_current_env(variable_name)?),
        }),
    }
}

fn describe_elapsed_time(earlier_time: SystemTime) -> String {
    let elapsed_duration = SystemTime::now()
        .duration_since(earlier_time)
        .unwrap_or_default();
    let rounded_duration = SignedDuration::try_from(elapsed_duration)
        .unwrap_or(SignedDuration::MAX)
        .round(Unit::Minute)
        .unwrap_or(SignedDuration::MAX);
    SpanPrinter::new().duration_to_string(&rounded_duration)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use tempfile::TempDir;

    use super::*;
    use crate::target_reading::fingerprint_units::{
        fingerprint_fixtures::{FixtureEnvValue, FixtureUnit, write_fixture_units},
        scan_fingerprint_units,
    };

    #[test]
    fn groups_crates_watching_same_drifted_variable() {
        let temporary_target = TempDir::new().unwrap();
        let recorded_env_values = [
            FixtureEnvValue {
                variable_name: "BINDGEN_EXTRA_CLANG_ARGS",
                recorded_value: "-O2 -frandom-seed=aaa",
            },
            FixtureEnvValue {
                variable_name: "STABLE_VAR",
                recorded_value: "same",
            },
        ];
        write_fixture_units(
            temporary_target.path(),
            &[
                FixtureUnit {
                    unit_directory: "pipewire-sys-1111111111111111",
                    unit_kind: "run-build-script-build-script-build",
                    env_values: &recorded_env_values,
                    ..FixtureUnit::default()
                },
                FixtureUnit {
                    unit_directory: "libspa-sys-1111111111111111",
                    unit_kind: "run-build-script-build-script-build",
                    env_values: &recorded_env_values,
                    ..FixtureUnit::default()
                },
            ],
        );
        let current_env = HashMap::from([
            ("BINDGEN_EXTRA_CLANG_ARGS", "-O2 -frandom-seed=bbb"),
            ("STABLE_VAR", "same"),
        ]);

        let rebuild_triggers = detect_recorded_env_drift(
            &scan_fingerprint_units(temporary_target.path()).unwrap(),
            &ProjectEnvironment::default(),
            |variable_name| current_env.get(variable_name).map(ToString::to_string),
        );

        assert_eq!(rebuild_triggers.len(), 1, "only the seed variable drifted");
        let seed_trigger = &rebuild_triggers[0];
        assert_eq!(seed_trigger.trigger_label, "$BINDGEN_EXTRA_CLANG_ARGS");
        assert_eq!(
            seed_trigger.trigger_change.to_string(),
            "-frandom-seed=aaa -> -frandom-seed=bbb"
        );
        assert_eq!(
            seed_trigger
                .observing_crates
                .iter()
                .map(|observing_crate| observing_crate.crate_name.as_str())
                .collect::<Vec<_>>(),
            vec!["libspa-sys", "pipewire-sys"]
        );
        assert!(
            seed_trigger.evidence.contains("0s ago"),
            "{}",
            seed_trigger.evidence
        );
        assert!(
            seed_trigger.fix_hints[0].summary.contains("-frandom-seed"),
            "nix hint"
        );
    }

    #[test]
    fn uses_newest_build_script_run_per_crate() {
        let temporary_target = TempDir::new().unwrap();
        write_fixture_units(
            temporary_target.path(),
            &[
                FixtureUnit {
                    unit_directory: "probe-1111111111111111",
                    unit_kind: "run-build-script-build-script-build",
                    env_values: &[FixtureEnvValue {
                        variable_name: "PROBE",
                        recorded_value: "old",
                    }],
                    ..FixtureUnit::default()
                },
                FixtureUnit {
                    unit_directory: "probe-2222222222222222",
                    unit_kind: "run-build-script-build-script-build",
                    env_values: &[FixtureEnvValue {
                        variable_name: "PROBE",
                        recorded_value: "current",
                    }],
                    ..FixtureUnit::default()
                },
            ],
        );
        let rebuild_triggers = detect_recorded_env_drift(
            &scan_fingerprint_units(temporary_target.path()).unwrap(),
            &ProjectEnvironment::default(),
            |_| Some("current".to_string()),
        );
        assert!(rebuild_triggers.is_empty(), "stale unit ignored");
    }

    #[test]
    fn reports_variable_that_became_set() {
        let temporary_target = TempDir::new().unwrap();
        write_fixture_units(
            temporary_target.path(),
            &[FixtureUnit {
                unit_directory: "probe-1111111111111111",
                unit_kind: "run-build-script-build-script-build",
                unset_env_variables: &["LIBCLANG_PATH"],
                ..FixtureUnit::default()
            }],
        );
        let rebuild_triggers = detect_recorded_env_drift(
            &scan_fingerprint_units(temporary_target.path()).unwrap(),
            &ProjectEnvironment::default(),
            |_| Some("/clang".to_string()),
        );
        assert_eq!(
            rebuild_triggers[0].trigger_change.to_string(),
            "unset -> /clang"
        );
    }
}
