use std::{
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

use log::debug;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{AnalyzerError, build_script_rerun_directives::strip_hash_suffix};

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct FingerprintDependencyEdge {
    pub dependency_name: String,
    pub fingerprint_hash: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordedEnvValue {
    pub variable_name: String,
    pub recorded_value: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FingerprintUnitRecord {
    pub profile_directory: String,
    pub crate_name: String,
    pub unit_directory: String,
    pub unit_kind: String,
    pub own_fingerprint_hash: Option<u64>,
    pub features: String,
    pub rustc_hash: u64,
    pub target_hash: u64,
    pub profile_hash: u64,
    pub source_path_hash: u64,
    pub cargo_config_hash: u64,
    pub compile_kind_hash: u64,
    pub rustflags: Vec<String>,
    pub dependency_edges: Vec<FingerprintDependencyEdge>,
    pub recorded_env_values: Vec<RecordedEnvValue>,
    #[serde(skip)]
    pub modified_time: SystemTime,
}

impl FingerprintUnitRecord {
    #[must_use]
    pub fn runs_build_script(&self) -> bool {
        self.unit_kind.starts_with("run-build-script")
    }

    #[must_use]
    pub fn feature_names(&self) -> Vec<String> {
        serde_json::from_str(&self.features).unwrap_or_default()
    }
}

#[derive(Deserialize)]
struct RawFingerprintJson {
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
    deps: Vec<(Value, String, Value, u64)>,
    #[serde(default)]
    local: Vec<Value>,
}

pub fn profile_directories_containing(
    target_dir: &Path,
    marker_subdirectory: &str,
) -> Result<Vec<PathBuf>, AnalyzerError> {
    let directory_entries = fs::read_dir(target_dir).map_err(|error| {
        AnalyzerError::BuildScriptOutputUnreadable(target_dir.to_path_buf(), error)
    })?;

    let mut profile_directories: Vec<PathBuf> = directory_entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && path.join(marker_subdirectory).is_dir())
        .collect();
    profile_directories.sort();
    Ok(profile_directories)
}

pub fn scan_fingerprint_units(
    target_dir: &Path,
) -> Result<Vec<FingerprintUnitRecord>, AnalyzerError> {
    let mut unit_records = Vec::new();
    for profile_directory in profile_directories_containing(target_dir, ".fingerprint")? {
        let profile_name = profile_directory
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string();
        let fingerprint_root = profile_directory.join(".fingerprint");
        let unit_directories = fs::read_dir(&fingerprint_root).map_err(|error| {
            AnalyzerError::BuildScriptOutputUnreadable(fingerprint_root.clone(), error)
        })?;
        for unit_directory in unit_directories
            .filter_map(Result::ok)
            .map(|entry| entry.path())
        {
            unit_records.extend(read_unit_directory(&profile_name, &unit_directory));
        }
    }
    Ok(unit_records)
}

fn read_unit_directory(profile_name: &str, unit_directory: &Path) -> Vec<FingerprintUnitRecord> {
    let Some(unit_directory_name) = unit_directory.file_name().and_then(|name| name.to_str())
    else {
        return Vec::new();
    };
    let Some(crate_name) = strip_hash_suffix(unit_directory_name) else {
        return Vec::new();
    };
    let Ok(unit_files) = fs::read_dir(unit_directory) else {
        return Vec::new();
    };
    unit_files
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .filter_map(|json_path| {
            read_unit_json(profile_name, crate_name, unit_directory_name, &json_path)
        })
        .collect()
}

fn read_unit_json(
    profile_name: &str,
    crate_name: &str,
    unit_directory_name: &str,
    json_path: &Path,
) -> Option<FingerprintUnitRecord> {
    let unit_kind = json_path.file_stem()?.to_str()?.to_string();
    let json_contents = fs::read_to_string(json_path).ok()?;
    let raw_fingerprint: RawFingerprintJson = serde_json::from_str(&json_contents)
        .inspect_err(|error| debug!("skipping {}: {error}", json_path.display()))
        .ok()?;
    let modified_time = fs::metadata(json_path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let own_fingerprint_hash = fs::read_to_string(json_path.with_extension(""))
        .ok()
        .and_then(|hash_contents| decode_little_endian_fingerprint_hash(hash_contents.trim()));

    Some(FingerprintUnitRecord {
        profile_directory: profile_name.to_string(),
        crate_name: crate_name.to_string(),
        unit_directory: unit_directory_name.to_string(),
        unit_kind,
        own_fingerprint_hash,
        features: raw_fingerprint.features,
        rustc_hash: raw_fingerprint.rustc,
        target_hash: raw_fingerprint.target,
        profile_hash: raw_fingerprint.profile,
        source_path_hash: raw_fingerprint.path,
        cargo_config_hash: raw_fingerprint.config,
        compile_kind_hash: raw_fingerprint.compile_kind,
        rustflags: raw_fingerprint.rustflags,
        dependency_edges: raw_fingerprint
            .deps
            .into_iter()
            .map(
                |(_, dependency_name, _, fingerprint_hash)| FingerprintDependencyEdge {
                    dependency_name,
                    fingerprint_hash,
                },
            )
            .collect(),
        recorded_env_values: raw_fingerprint
            .local
            .iter()
            .filter_map(extract_recorded_env_value)
            .collect(),
        modified_time,
    })
}

fn extract_recorded_env_value(local_entry: &Value) -> Option<RecordedEnvValue> {
    let env_entry = local_entry.get("RerunIfEnvChanged")?;
    Some(RecordedEnvValue {
        variable_name: env_entry.get("var")?.as_str()?.to_string(),
        recorded_value: env_entry
            .get("val")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

#[must_use]
pub fn decode_little_endian_fingerprint_hash(hex_contents: &str) -> Option<u64> {
    if hex_contents.len() != 16 || !hex_contents.is_ascii() {
        return None;
    }
    let mut hash_bytes = [0_u8; 8];
    for (byte_slot, hex_pair) in hash_bytes.iter_mut().zip(hex_contents.as_bytes().chunks(2)) {
        *byte_slot = u8::from_str_radix(str::from_utf8(hex_pair).ok()?, 16).ok()?;
    }
    Some(u64::from_le_bytes(hash_bytes))
}

#[cfg(test)]
pub mod fingerprint_fixture_writer {
    use std::{fmt::Write, fs, path::Path};

    use serde_json::json;

    pub struct FixtureUnit<'fixture> {
        pub unit_directory: &'fixture str,
        pub unit_kind: &'fixture str,
        pub own_fingerprint_hash: u64,
        pub features: &'fixture [&'fixture str],
        pub profile_hash: u64,
        pub source_path_hash: u64,
        pub dependency_edges: &'fixture [(&'fixture str, u64)],
        pub recorded_env_values: &'fixture [(&'fixture str, Option<&'fixture str>)],
    }

    impl Default for FixtureUnit<'_> {
        fn default() -> Self {
            Self {
                unit_directory: "placeholder-0000000000000000",
                unit_kind: "lib-placeholder",
                own_fingerprint_hash: 0,
                features: &[],
                profile_hash: 1,
                source_path_hash: 1,
                dependency_edges: &[],
                recorded_env_values: &[],
            }
        }
    }

    pub fn write_fixture_unit(target_dir: &Path, fixture_unit: &FixtureUnit<'_>) {
        let unit_path = target_dir
            .join("debug")
            .join(".fingerprint")
            .join(fixture_unit.unit_directory);
        fs::create_dir_all(&unit_path).unwrap();
        let fingerprint_json = json!({
            "rustc": 7,
            "features": serde_json::to_string(fixture_unit.features).unwrap(),
            "declared_features": "[]",
            "target": 3,
            "profile": fixture_unit.profile_hash,
            "path": fixture_unit.source_path_hash,
            "deps": fixture_unit
                .dependency_edges
                .iter()
                .map(|(dependency_name, fingerprint_hash)| json!([0, dependency_name, false, fingerprint_hash]))
                .collect::<Vec<_>>(),
            "local": fixture_unit
                .recorded_env_values
                .iter()
                .map(|(variable_name, recorded_value)| json!({"RerunIfEnvChanged": {"var": variable_name, "val": recorded_value}}))
                .collect::<Vec<_>>(),
            "rustflags": [],
            "config": 0,
            "compile_kind": 0,
        });
        fs::write(
            unit_path.join(format!("{}.json", fixture_unit.unit_kind)),
            fingerprint_json.to_string(),
        )
        .unwrap();
        let encoded_hash = fixture_unit.own_fingerprint_hash.to_le_bytes().iter().fold(
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
        fingerprint_fixture_writer::{FixtureUnit, write_fixture_unit},
        *,
    };

    #[test]
    fn decodes_hash_written_by_cargo() {
        assert_eq!(
            decode_little_endian_fingerprint_hash("f4329112e254ae04"),
            Some(337_300_352_050_213_620)
        );
        assert_eq!(decode_little_endian_fingerprint_hash("short"), None);
    }

    #[test]
    fn reads_dependencies_and_recorded_env_values() {
        let temporary_target = TempDir::new().unwrap();
        write_fixture_unit(
            temporary_target.path(),
            &FixtureUnit {
                unit_directory: "pipewire-sys-0123456789abcdef",
                unit_kind: "run-build-script-build-script-build",
                own_fingerprint_hash: 42,
                dependency_edges: &[("bindgen", 99)],
                recorded_env_values: &[("PKG_CONFIG_PATH", Some("/a:/b")), ("CC", None)],
                ..FixtureUnit::default()
            },
        );

        let unit_records = scan_fingerprint_units(temporary_target.path()).unwrap();
        assert_eq!(unit_records.len(), 1, "one unit json expected");
        let pipewire_record = &unit_records[0];
        assert_eq!(pipewire_record.crate_name, "pipewire-sys");
        assert!(pipewire_record.runs_build_script(), "run-build-script kind");
        assert_eq!(pipewire_record.own_fingerprint_hash, Some(42));
        assert_eq!(
            pipewire_record.dependency_edges,
            vec![FingerprintDependencyEdge {
                dependency_name: "bindgen".to_string(),
                fingerprint_hash: 99
            }]
        );
        assert_eq!(pipewire_record.recorded_env_values.len(), 2);
        assert_eq!(
            pipewire_record.recorded_env_values[1].recorded_value, None,
            "unset values stay None"
        );
    }

    #[test]
    fn parses_feature_list() {
        let temporary_target = TempDir::new().unwrap();
        write_fixture_unit(
            temporary_target.path(),
            &FixtureUnit {
                unit_directory: "syn-0123456789abcdef",
                unit_kind: "lib-syn",
                features: &["full", "visit"],
                ..FixtureUnit::default()
            },
        );
        let unit_records = scan_fingerprint_units(temporary_target.path()).unwrap();
        assert_eq!(unit_records[0].feature_names(), vec!["full", "visit"]);
    }
}
