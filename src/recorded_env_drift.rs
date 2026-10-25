use std::{
    collections::{BTreeMap, BTreeSet},
    time::SystemTime,
};

use serde::Serialize;

use crate::{
    build_script_rerun_directives::is_volatile,
    env_value_token_diff::EnvValueTokenDiff,
    fingerprint_unit_records::FingerprintUnitRecord,
    fix_hint_library::{FixHint, hints_for_env_change},
};

#[derive(Debug, Clone, Serialize)]
pub struct RecordedEnvDrift {
    pub variable_name: String,
    pub watching_crates: Vec<String>,
    pub token_diff: EnvValueTokenDiff,
    pub volatile: bool,
    pub newest_recorded_run_seconds_ago: u64,
    pub fix_hints: Vec<FixHint>,
}

#[must_use]
pub fn detect_recorded_env_drift(
    unit_records: &[FingerprintUnitRecord],
    lookup_current_env: impl Fn(&str) -> Option<String>,
) -> Vec<RecordedEnvDrift> {
    let mut newest_run_by_crate: BTreeMap<(&str, &str), &FingerprintUnitRecord> = BTreeMap::new();
    for unit_record in unit_records
        .iter()
        .filter(|unit_record| unit_record.runs_build_script())
    {
        newest_run_by_crate
            .entry((&unit_record.profile_directory, &unit_record.crate_name))
            .and_modify(|newest_record| {
                if unit_record.modified_time > newest_record.modified_time {
                    *newest_record = unit_record;
                }
            })
            .or_insert(unit_record);
    }

    let mut crates_by_drift: BTreeMap<(&str, EnvValueTokenDiff), (BTreeSet<&str>, SystemTime)> =
        BTreeMap::new();
    for build_script_run in newest_run_by_crate.values() {
        for recorded_env_value in &build_script_run.recorded_env_values {
            let current_value = lookup_current_env(&recorded_env_value.variable_name);
            if let Some(token_diff) = EnvValueTokenDiff::between(
                recorded_env_value.recorded_value.as_deref(),
                current_value.as_deref(),
            ) {
                let (watching_crates, newest_recorded_run) = crates_by_drift
                    .entry((&recorded_env_value.variable_name, token_diff))
                    .or_insert_with(|| (BTreeSet::new(), SystemTime::UNIX_EPOCH));
                watching_crates.insert(&build_script_run.crate_name);
                *newest_recorded_run = (*newest_recorded_run).max(build_script_run.modified_time);
            }
        }
    }

    let mut env_drifts: Vec<RecordedEnvDrift> = crates_by_drift
        .into_iter()
        .map(
            |((variable_name, token_diff), (watching_crates, newest_recorded_run))| {
                RecordedEnvDrift {
                    fix_hints: hints_for_env_change(variable_name, &token_diff),
                    volatile: is_volatile(variable_name),
                    newest_recorded_run_seconds_ago: SystemTime::now()
                        .duration_since(newest_recorded_run)
                        .unwrap_or_default()
                        .as_secs(),
                    variable_name: variable_name.to_string(),
                    watching_crates: watching_crates.into_iter().map(str::to_string).collect(),
                    token_diff,
                }
            },
        )
        .collect();
    env_drifts.sort_by(|left_drift, right_drift| {
        right_drift
            .watching_crates
            .len()
            .cmp(&left_drift.watching_crates.len())
            .then_with(|| left_drift.variable_name.cmp(&right_drift.variable_name))
    });
    env_drifts
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, thread, time::Duration};

    use tempfile::TempDir;

    use super::*;
    use crate::fingerprint_unit_records::{
        fingerprint_fixture_writer::{FixtureUnit, write_fixture_unit},
        scan_fingerprint_units,
    };

    #[test]
    fn groups_crates_watching_same_drifted_variable() {
        let temporary_target = TempDir::new().unwrap();
        let recorded_args = Some("-O2 -frandom-seed=aaa");
        for unit_directory in [
            "pipewire-sys-1111111111111111",
            "libspa-sys-1111111111111111",
        ] {
            write_fixture_unit(
                temporary_target.path(),
                &FixtureUnit {
                    unit_directory,
                    unit_kind: "run-build-script-build-script-build",
                    recorded_env_values: &[
                        ("BINDGEN_EXTRA_CLANG_ARGS", recorded_args),
                        ("STABLE_VAR", Some("same")),
                    ],
                    ..FixtureUnit::default()
                },
            );
        }
        let current_env = HashMap::from([
            ("BINDGEN_EXTRA_CLANG_ARGS", "-O2 -frandom-seed=bbb"),
            ("STABLE_VAR", "same"),
        ]);

        let unit_records = scan_fingerprint_units(temporary_target.path()).unwrap();
        let env_drifts = detect_recorded_env_drift(&unit_records, |variable_name| {
            current_env
                .get(variable_name)
                .map(|value| (*value).to_string())
        });

        assert_eq!(
            env_drifts.len(),
            1,
            "only the seed variable drifted: {env_drifts:?}"
        );
        assert_eq!(env_drifts[0].variable_name, "BINDGEN_EXTRA_CLANG_ARGS");
        assert_eq!(
            env_drifts[0].watching_crates,
            vec!["libspa-sys", "pipewire-sys"]
        );
        assert!(
            env_drifts[0].fix_hints[0].summary.contains("frandom-seed"),
            "nix hint"
        );
    }

    #[test]
    fn uses_newest_build_script_run_per_crate() {
        let temporary_target = TempDir::new().unwrap();
        write_fixture_unit(
            temporary_target.path(),
            &FixtureUnit {
                unit_directory: "foo-1111111111111111",
                unit_kind: "run-build-script-build-script-build",
                recorded_env_values: &[("PROBE", Some("old"))],
                ..FixtureUnit::default()
            },
        );
        thread::sleep(Duration::from_millis(15));
        write_fixture_unit(
            temporary_target.path(),
            &FixtureUnit {
                unit_directory: "foo-2222222222222222",
                unit_kind: "run-build-script-build-script-build",
                recorded_env_values: &[("PROBE", Some("current"))],
                ..FixtureUnit::default()
            },
        );

        let unit_records = scan_fingerprint_units(temporary_target.path()).unwrap();
        let env_drifts = detect_recorded_env_drift(&unit_records, |_| Some("current".to_string()));
        assert!(env_drifts.is_empty(), "stale unit ignored: {env_drifts:?}");
    }
}
