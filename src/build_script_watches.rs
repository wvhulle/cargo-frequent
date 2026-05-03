//! Parses `cargo:rerun-if-env-changed` directives from build-script output
//! files written by cargo into `target/<profile>/build/<crate-hash>/output`.
//!
//! Cross-referenced against a curated list of "volatile" env vars (PATH,
//! HOME, XDG_*, …) so the report can highlight build scripts that watch
//! values that vary between shells/sessions and therefore guarantee
//! rebuilds.
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde::Serialize;

use crate::AnalyzerError;

const VOLATILE_EXACT: &[&str] = &[
    "PATH",
    "PWD",
    "OLDPWD",
    "SHLVL",
    "SHELL",
    "TERM",
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "TERM_SESSION_ID",
    "HOME",
    "USER",
    "LOGNAME",
    "TMPDIR",
    "VIRTUAL_ENV",
    "VIRTUAL_ENV_PROMPT",
    "_",
    "OLDOLDPWD",
];

const VOLATILE_PREFIXES: &[&str] = &[
    "XDG_",
    "CONDA_",
    "DIRENV_",
    "SSH_",
    "ITERM_",
    "VSCODE_",
    "TERMINFO_",
];

/// Returns true if a given env var name is on the curated volatile list.
#[must_use]
pub fn is_volatile(var_name: &str) -> bool {
    VOLATILE_EXACT.contains(&var_name)
        || VOLATILE_PREFIXES
            .iter()
            .any(|prefix| var_name.starts_with(prefix))
}

/// Env vars and rerun-if-changed paths watched by a single crate's build
/// script.
#[derive(Debug, Clone, Default, Serialize)]
pub struct BuildScriptWatches {
    pub env_vars: Vec<String>,
    pub paths: Vec<String>,
}

/// Map from crate name (e.g. "pyo3-build-config") to the directives its
/// build script emitted on the most recent run.
pub type WatchMap = BTreeMap<String, BuildScriptWatches>;

/// Scan the build-script output directory of a cargo target dir and return
/// every `cargo:rerun-if-*` directive grouped by crate name.
///
/// Cargo writes one `output` file per build-script run at:
/// `<target_dir>/<profile>/build/<crate>-<hash>/output`. We parse each one.
pub fn scan(target_dir: &Path) -> Result<WatchMap, AnalyzerError> {
    let mut map = WatchMap::new();

    for profile_dir in profile_dirs(target_dir)? {
        let build_dir = profile_dir.join("build");
        if !build_dir.is_dir() {
            continue;
        }

        let entries = fs::read_dir(&build_dir)
            .map_err(|e| AnalyzerError::BuildScriptOutputUnreadable(build_dir.clone(), e))?;

        for entry in entries {
            let entry =
                entry.map_err(|e| AnalyzerError::BuildScriptOutputUnreadable(build_dir.clone(), e))?;
            let crate_dir = entry.path();
            let output_file = crate_dir.join("output");
            if !output_file.is_file() {
                continue;
            }

            let crate_name = crate_dir
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(strip_hash_suffix)
                .unwrap_or_default()
                .to_string();
            if crate_name.is_empty() {
                continue;
            }

            let contents = fs::read_to_string(&output_file)
                .map_err(|e| AnalyzerError::BuildScriptOutputUnreadable(output_file.clone(), e))?;

            let watches = map.entry(crate_name).or_default();
            extend_from_output(watches, &contents);
        }
    }

    for watches in map.values_mut() {
        watches.env_vars.sort();
        watches.env_vars.dedup();
        watches.paths.sort();
        watches.paths.dedup();
    }

    Ok(map)
}

fn profile_dirs(target_dir: &Path) -> Result<Vec<PathBuf>, AnalyzerError> {
    let read = fs::read_dir(target_dir)
        .map_err(|e| AnalyzerError::BuildScriptOutputUnreadable(target_dir.to_path_buf(), e))?;

    read.filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && path.join("build").is_dir())
        .map(Ok)
        .collect()
}

fn extend_from_output(watches: &mut BuildScriptWatches, contents: &str) {
    for line in contents.lines() {
        if let Some(var) = line.strip_prefix("cargo:rerun-if-env-changed=") {
            watches.env_vars.push(var.trim().to_string());
        } else if let Some(path) = line.strip_prefix("cargo:rerun-if-changed=") {
            watches.paths.push(path.trim().to_string());
        }
    }
}

/// `pyo3-build-config-abc123def` → `pyo3-build-config`. Cargo always
/// suffixes a 16-hex-char hash; strip it conservatively.
fn strip_hash_suffix(dir_name: &str) -> Option<&str> {
    let (name, hash) = dir_name.rsplit_once('-')?;
    if hash.len() == 16 && hash.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(name)
    } else {
        Some(dir_name)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn detects_volatile_vars() {
        assert!(is_volatile("PATH"));
        assert!(is_volatile("HOME"));
        assert!(is_volatile("XDG_CONFIG_HOME"));
        assert!(is_volatile("CONDA_PREFIX"));
        assert!(!is_volatile("CC"));
        assert!(!is_volatile("RUSTFLAGS"));
        assert!(!is_volatile("PYO3_PYTHON"));
    }

    #[test]
    fn parses_directives_from_output_file() {
        let temp = TempDir::new().unwrap();
        let build_dir = temp.path().join("debug").join("build");
        let crate_dir = build_dir.join("pyo3-build-config-0123456789abcdef");
        fs::create_dir_all(&crate_dir).unwrap();
        fs::write(
            crate_dir.join("output"),
            "cargo:rerun-if-env-changed=PATH\n\
             cargo:rerun-if-env-changed=PYO3_PYTHON\n\
             cargo:rerun-if-changed=build.rs\n\
             cargo:rustc-cfg=Py_3_8\n",
        )
        .unwrap();

        let map = scan(temp.path()).unwrap();
        let watches = map.get("pyo3-build-config").expect("crate present");
        assert_eq!(watches.env_vars, vec!["PATH", "PYO3_PYTHON"]);
        assert_eq!(watches.paths, vec!["build.rs"]);
    }

    #[test]
    fn merges_directives_across_profiles() {
        let temp = TempDir::new().unwrap();
        for profile in ["debug", "release"] {
            let crate_dir = temp
                .path()
                .join(profile)
                .join("build")
                .join("foo-aaaaaaaaaaaaaaaa");
            fs::create_dir_all(&crate_dir).unwrap();
            fs::write(
                crate_dir.join("output"),
                format!("cargo:rerun-if-env-changed={}\n", profile.to_uppercase()),
            )
            .unwrap();
        }

        let map = scan(temp.path()).unwrap();
        let watches = map.get("foo").expect("foo present");
        assert!(watches.env_vars.contains(&"DEBUG".to_string()));
        assert!(watches.env_vars.contains(&"RELEASE".to_string()));
    }

    #[test]
    fn empty_target_dir_yields_empty_map() {
        let temp = TempDir::new().unwrap();
        let map = scan(temp.path()).unwrap();
        assert!(map.is_empty());
    }
}
