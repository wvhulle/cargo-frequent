use std::{
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::frequent_error::FrequentError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub enum UnitRole {
    BuildScriptRun,
    BuildScriptCompile,
    CrateCompile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DependencyEdge {
    pub dependency_name: String,
    pub fingerprint_hash: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum RecordedEnvValue {
    Set {
        variable_name: String,
        value: String,
    },
    Unset {
        variable_name: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct FingerprintUnit {
    pub profile_directory: String,
    pub crate_name: String,
    pub unit_directory: String,
    pub unit_kind: String,
    pub unit_role: UnitRole,
    pub fingerprint_hash: u64,
    pub feature_names: Vec<String>,
    pub rustc_hash: u64,
    pub target_hash: u64,
    pub profile_hash: u64,
    pub source_path_hash: u64,
    pub cargo_config_hash: u64,
    pub compile_kind_hash: u64,
    pub rustflags: Vec<String>,
    pub dependency_edges: Vec<DependencyEdge>,
    pub recorded_env_values: Vec<RecordedEnvValue>,
    #[serde(skip)]
    pub modified_time: SystemTime,
}

#[derive(Deserialize)]
struct FingerprintJson {
    rustc: u64,
    features: String,
    target: u64,
    profile: u64,
    path: u64,
    #[serde(default)]
    config: u64,
    #[serde(default)]
    compile_kind: u64,
    #[serde(default)]
    rustflags: Vec<String>,
    #[serde(default)]
    deps: Vec<Vec<Value>>,
    #[serde(default)]
    local: Vec<Value>,
}

const UNIT_HASH_LENGTH: usize = 16;

pub fn scan_fingerprint_units(
    target_directory: &Path,
) -> Result<Vec<FingerprintUnit>, FrequentError> {
    let profile_unit_lists: Vec<Vec<FingerprintUnit>> = read_directory_paths(target_directory)?
        .into_iter()
        .filter(|profile_directory| profile_directory.join(".fingerprint").is_dir())
        .map(|profile_directory| read_profile_units(&profile_directory))
        .collect::<Result<_, _>>()?;
    Ok(profile_unit_lists.into_iter().flatten().collect())
}

fn read_directory_paths(directory_path: &Path) -> Result<Vec<PathBuf>, FrequentError> {
    let directory_entries =
        fs::read_dir(directory_path).map_err(|io_error| FrequentError::DirectoryUnreadable {
            directory_path: directory_path.to_path_buf(),
            io_error,
        })?;
    Ok(directory_entries
        .filter_map(Result::ok)
        .map(|directory_entry| directory_entry.path())
        .collect())
}

fn read_profile_units(profile_directory: &Path) -> Result<Vec<FingerprintUnit>, FrequentError> {
    let profile_name = profile_directory
        .file_name()
        .and_then(|directory_name| directory_name.to_str())
        .unwrap_or_default();
    Ok(
        read_directory_paths(&profile_directory.join(".fingerprint"))?
            .iter()
            .flat_map(|unit_directory| read_unit_directory(profile_name, unit_directory))
            .collect(),
    )
}

fn read_unit_directory(profile_name: &str, unit_directory: &Path) -> Vec<FingerprintUnit> {
    let Some(unit_directory_name) = unit_directory
        .file_name()
        .and_then(|directory_name| directory_name.to_str())
    else {
        return Vec::new();
    };
    let Some((crate_name, unit_hash)) = unit_directory_name.rsplit_once('-') else {
        return Vec::new();
    };
    if unit_hash.len() != UNIT_HASH_LENGTH
        || !unit_hash
            .chars()
            .all(|hash_character| hash_character.is_ascii_hexdigit())
    {
        return Vec::new();
    }
    let Ok(unit_files) = fs::read_dir(unit_directory) else {
        return Vec::new();
    };
    unit_files
        .filter_map(Result::ok)
        .map(|unit_file| unit_file.path())
        .filter(|unit_file_path| {
            unit_file_path
                .extension()
                .is_some_and(|extension| extension == "json")
        })
        .filter_map(|json_path| {
            read_unit_json(
                &UnitLocation {
                    profile_directory: profile_name,
                    crate_name,
                    unit_directory: unit_directory_name,
                },
                &json_path,
            )
        })
        .collect()
}

struct UnitLocation<'location> {
    profile_directory: &'location str,
    crate_name: &'location str,
    unit_directory: &'location str,
}

fn read_unit_json(unit_location: &UnitLocation<'_>, json_path: &Path) -> Option<FingerprintUnit> {
    let unit_kind = json_path.file_stem()?.to_str()?.to_string();
    let fingerprint_hash = decode_little_endian_hash(
        fs::read_to_string(json_path.with_extension(""))
            .ok()?
            .trim(),
    )?;
    let fingerprint_json: FingerprintJson =
        serde_json::from_str(&fs::read_to_string(json_path).ok()?).ok()?;
    let modified_time = fs::metadata(json_path).ok()?.modified().ok()?;
    let unit_role = if unit_kind.starts_with("run-build-script") {
        UnitRole::BuildScriptRun
    } else if unit_kind.starts_with("build-script") {
        UnitRole::BuildScriptCompile
    } else {
        UnitRole::CrateCompile
    };

    Some(FingerprintUnit {
        profile_directory: unit_location.profile_directory.to_string(),
        crate_name: unit_location.crate_name.to_string(),
        unit_directory: unit_location.unit_directory.to_string(),
        unit_kind,
        unit_role,
        fingerprint_hash,
        feature_names: serde_json::from_str(&fingerprint_json.features).unwrap_or_default(),
        rustc_hash: fingerprint_json.rustc,
        target_hash: fingerprint_json.target,
        profile_hash: fingerprint_json.profile,
        source_path_hash: fingerprint_json.path,
        cargo_config_hash: fingerprint_json.config,
        compile_kind_hash: fingerprint_json.compile_kind,
        rustflags: fingerprint_json.rustflags,
        dependency_edges: fingerprint_json
            .deps
            .iter()
            .map(Vec::as_slice)
            .filter_map(read_dependency_edge)
            .collect(),
        recorded_env_values: fingerprint_json
            .local
            .iter()
            .filter_map(read_recorded_env_value)
            .collect(),
        modified_time,
    })
}

fn read_dependency_edge(dependency_entry: &[Value]) -> Option<DependencyEdge> {
    Some(DependencyEdge {
        dependency_name: dependency_entry.get(1)?.as_str()?.to_string(),
        fingerprint_hash: dependency_entry.get(3)?.as_u64()?,
    })
}

fn read_recorded_env_value(local_entry: &Value) -> Option<RecordedEnvValue> {
    let env_entry = local_entry.get("RerunIfEnvChanged")?;
    let variable_name = env_entry.get("var")?.as_str()?.to_string();
    let Some(value) = env_entry.get("val").and_then(Value::as_str) else {
        return Some(RecordedEnvValue::Unset { variable_name });
    };
    Some(RecordedEnvValue::Set {
        variable_name,
        value: value.to_string(),
    })
}

fn decode_little_endian_hash(hex_contents: &str) -> Option<u64> {
    if hex_contents.len() != UNIT_HASH_LENGTH || !hex_contents.is_ascii() {
        return None;
    }
    let hash_bytes = hex_contents
        .as_bytes()
        .chunks(2)
        .map(|hex_pair| u8::from_str_radix(str::from_utf8(hex_pair).ok()?, 16).ok())
        .collect::<Option<Vec<u8>>>()?;
    Some(u64::from_le_bytes(hash_bytes.try_into().ok()?))
}

#[cfg(test)]
pub mod fingerprint_fixtures {
    use std::{fmt::Write, fs, path::Path, thread, time::Duration};

    use serde_json::json;

    pub struct FixtureDependency<'fixture> {
        pub dependency_name: &'fixture str,
        pub fingerprint_hash: u64,
    }

    pub struct FixtureEnvValue<'fixture> {
        pub variable_name: &'fixture str,
        pub recorded_value: &'fixture str,
    }

    pub struct FixtureUnit<'fixture> {
        pub unit_directory: &'fixture str,
        pub unit_kind: &'fixture str,
        pub fingerprint_hash: u64,
        pub feature_names: &'fixture [&'fixture str],
        pub source_path_hash: u64,
        pub dependencies: &'fixture [FixtureDependency<'fixture>],
        pub env_values: &'fixture [FixtureEnvValue<'fixture>],
        pub unset_env_variables: &'fixture [&'fixture str],
    }

    impl Default for FixtureUnit<'_> {
        fn default() -> Self {
            Self {
                unit_directory: "placeholder-0000000000000000",
                unit_kind: "lib-placeholder",
                fingerprint_hash: 0,
                feature_names: &[],
                source_path_hash: 1,
                dependencies: &[],
                env_values: &[],
                unset_env_variables: &[],
            }
        }
    }

    pub fn write_fixture_units(target_directory: &Path, fixture_units: &[FixtureUnit<'_>]) {
        fixture_units.iter().for_each(|fixture_unit| {
            write_fixture_unit(target_directory, fixture_unit);
            thread::sleep(Duration::from_millis(15));
        });
    }

    fn write_fixture_unit(target_directory: &Path, fixture_unit: &FixtureUnit<'_>) {
        let unit_path = target_directory
            .join("debug")
            .join(".fingerprint")
            .join(fixture_unit.unit_directory);
        fs::create_dir_all(&unit_path).unwrap();
        let local_entries: Vec<_> = fixture_unit
            .env_values
            .iter()
            .map(|env_value| json!({"RerunIfEnvChanged": {"var": env_value.variable_name, "val": env_value.recorded_value}}))
            .chain(
                fixture_unit
                    .unset_env_variables
                    .iter()
                    .map(|variable_name| json!({"RerunIfEnvChanged": {"var": variable_name, "val": null}})),
            )
            .collect();
        let dependency_entries: Vec<_> = fixture_unit
            .dependencies
            .iter()
            .map(|dependency| {
                json!([
                    0,
                    dependency.dependency_name,
                    false,
                    dependency.fingerprint_hash
                ])
            })
            .collect();
        let fingerprint_json = json!({
            "rustc": 7,
            "features": serde_json::to_string(fixture_unit.feature_names).unwrap(),
            "declared_features": "[]",
            "target": 3,
            "profile": 1,
            "path": fixture_unit.source_path_hash,
            "deps": dependency_entries,
            "local": local_entries,
            "rustflags": [],
            "config": 0,
            "compile_kind": 0,
        });
        fs::write(
            unit_path.join(format!("{}.json", fixture_unit.unit_kind)),
            fingerprint_json.to_string(),
        )
        .unwrap();
        let encoded_hash = fixture_unit.fingerprint_hash.to_le_bytes().iter().fold(
            String::new(),
            |mut encoded_text, hash_byte| {
                let _ = write!(encoded_text, "{hash_byte:02x}");
                encoded_text
            },
        );
        fs::write(unit_path.join(fixture_unit.unit_kind), encoded_hash).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::{
        fingerprint_fixtures::{
            FixtureDependency, FixtureEnvValue, FixtureUnit, write_fixture_units,
        },
        *,
    };

    #[test]
    fn decodes_hash_written_by_cargo() {
        assert_eq!(
            decode_little_endian_hash("f4329112e254ae04"),
            Some(337_300_352_050_213_620)
        );
        assert_eq!(decode_little_endian_hash("short"), None);
    }

    #[test]
    fn reads_dependencies_and_recorded_env_values() {
        let temporary_target = TempDir::new().unwrap();
        write_fixture_units(
            temporary_target.path(),
            &[FixtureUnit {
                unit_directory: "pipewire-sys-0123456789abcdef",
                unit_kind: "run-build-script-build-script-build",
                fingerprint_hash: 42,
                dependencies: &[FixtureDependency {
                    dependency_name: "bindgen",
                    fingerprint_hash: 99,
                }],
                env_values: &[FixtureEnvValue {
                    variable_name: "PKG_CONFIG_PATH",
                    recorded_value: "/a:/b",
                }],
                unset_env_variables: &["CC"],
                ..FixtureUnit::default()
            }],
        );

        let fingerprint_units = scan_fingerprint_units(temporary_target.path()).unwrap();
        assert_eq!(fingerprint_units.len(), 1, "one unit json expected");
        let pipewire_unit = &fingerprint_units[0];
        assert_eq!(pipewire_unit.crate_name, "pipewire-sys");
        assert_eq!(pipewire_unit.unit_role, UnitRole::BuildScriptRun);
        assert_eq!(pipewire_unit.fingerprint_hash, 42);
        assert_eq!(
            pipewire_unit.dependency_edges,
            vec![DependencyEdge {
                dependency_name: "bindgen".to_string(),
                fingerprint_hash: 99
            }]
        );
        assert_eq!(
            pipewire_unit.recorded_env_values[1],
            RecordedEnvValue::Unset {
                variable_name: "CC".to_string()
            }
        );
    }

    #[test]
    fn parses_feature_list() {
        let temporary_target = TempDir::new().unwrap();
        write_fixture_units(
            temporary_target.path(),
            &[FixtureUnit {
                unit_directory: "syn-0123456789abcdef",
                unit_kind: "lib-syn",
                feature_names: &["full", "visit"],
                ..FixtureUnit::default()
            }],
        );
        let fingerprint_units = scan_fingerprint_units(temporary_target.path()).unwrap();
        assert_eq!(fingerprint_units[0].feature_names, vec!["full", "visit"]);
    }
}
