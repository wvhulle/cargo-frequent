use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

use rnix::{Root, ast::AttrpathValue};
use rowan::ast::AstNode;
use serde::Serialize;

use crate::target_reading::{FileLocation, line_number_at, relative_display_path};

const DIRENV_CACHE_DIRECTORY: &str = ".direnv";
const NIX_SHELL_DEFINITIONS: &[&str] = &["flake.nix", "shell.nix", "default.nix", "devenv.nix"];
const NIX_DIRENV_WATCHED_FILES: &[&str] = &[
    "flake.nix",
    "flake.lock",
    "shell.nix",
    "default.nix",
    "devenv.nix",
    "devenv.lock",
    "devenv.yaml",
];
const CARGO_CONFIG_FILE_NAMES: &[&str] = &["config.toml", "config"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EnvrcDirective {
    pub line_number: usize,
    pub directive_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WatchedFile {
    pub relative_path: String,
    #[serde(skip)]
    pub modified_time: SystemTime,
}

#[derive(Debug, Clone, Serialize)]
pub struct DirenvConfiguration {
    pub envrc_directives: Vec<EnvrcDirective>,
    pub profile_store_paths: Vec<String>,
    pub exported_variables: BTreeSet<String>,
    pub watched_files: Vec<WatchedFile>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub enum DirenvSetup {
    #[default]
    Absent,
    Present(DirenvConfiguration),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum PythonEnvironmentKind {
    VirtualEnv,
    Conda,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PythonEnvironment {
    pub environment_kind: PythonEnvironmentKind,
    pub prefix_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VersionManager {
    pub manager_name: &'static str,
    pub config_file: String,
    pub path_fragment: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CargoEnvEntry {
    pub variable_name: String,
    pub configured_value: String,
    pub location: FileLocation,
}

impl ProjectEnvironment {
    #[must_use]
    pub fn configured_cargo_env_value(&self, variable_name: &str) -> Option<String> {
        self.cargo_env_entries
            .iter()
            .find(|cargo_env_entry| cargo_env_entry.variable_name == variable_name)
            .map(|cargo_env_entry| cargo_env_entry.configured_value.clone())
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ProjectEnvironment {
    pub direnv_setup: DirenvSetup,
    pub dev_shell_locations: Vec<FileLocation>,
    pub python_environments: Vec<PythonEnvironment>,
    pub version_managers: Vec<VersionManager>,
    pub cargo_env_entries: Vec<CargoEnvEntry>,
}

struct PythonEnvironmentMarker {
    marker_variable: &'static str,
    environment_kind: PythonEnvironmentKind,
}

const PYTHON_ENVIRONMENT_MARKERS: &[PythonEnvironmentMarker] = &[
    PythonEnvironmentMarker {
        marker_variable: "VIRTUAL_ENV",
        environment_kind: PythonEnvironmentKind::VirtualEnv,
    },
    PythonEnvironmentMarker {
        marker_variable: "CONDA_PREFIX",
        environment_kind: PythonEnvironmentKind::Conda,
    },
];

struct VersionManagerSignature {
    manager_name: &'static str,
    config_files: &'static [&'static str],
    path_fragment: &'static str,
}

const VERSION_MANAGER_SIGNATURES: &[VersionManagerSignature] = &[
    VersionManagerSignature {
        manager_name: "mise",
        config_files: &["mise.toml", ".mise.toml", ".config/mise.toml"],
        path_fragment: "/mise/",
    },
    VersionManagerSignature {
        manager_name: "asdf",
        config_files: &[".tool-versions"],
        path_fragment: "/.asdf/",
    },
    VersionManagerSignature {
        manager_name: "devbox",
        config_files: &["devbox.json"],
        path_fragment: "/.devbox/",
    },
    VersionManagerSignature {
        manager_name: "devenv",
        config_files: &["devenv.nix"],
        path_fragment: "/.devenv/",
    },
];

pub fn read_project_environment(
    project_path: &Path,
    lookup_current_env: impl Fn(&str) -> Option<String>,
) -> ProjectEnvironment {
    ProjectEnvironment {
        direnv_setup: read_direnv_setup(project_path),
        dev_shell_locations: locate_dev_shell_definitions(project_path),
        python_environments: read_python_environments(&lookup_current_env),
        version_managers: VERSION_MANAGER_SIGNATURES
            .iter()
            .flat_map(|manager_signature| {
                manager_signature
                    .config_files
                    .iter()
                    .filter(|config_file| project_path.join(config_file).is_file())
                    .map(|config_file| VersionManager {
                        manager_name: manager_signature.manager_name,
                        config_file: (*config_file).to_string(),
                        path_fragment: manager_signature.path_fragment,
                    })
            })
            .collect(),
        cargo_env_entries: read_cargo_env_entries(project_path),
    }
}

fn read_direnv_setup(project_path: &Path) -> DirenvSetup {
    let Ok(envrc_text) = fs::read_to_string(project_path.join(".envrc")) else {
        return DirenvSetup::Absent;
    };
    let envrc_directives: Vec<EnvrcDirective> = envrc_text
        .lines()
        .enumerate()
        .map(|(line_position, envrc_line)| EnvrcDirective {
            line_number: line_position + 1,
            directive_text: envrc_line.trim().to_string(),
        })
        .filter(|envrc_directive| {
            !envrc_directive.directive_text.is_empty()
                && !envrc_directive.directive_text.starts_with('#')
        })
        .collect();
    let explicitly_watched_files = envrc_directives
        .iter()
        .filter_map(|envrc_directive| envrc_directive.directive_text.strip_prefix("watch_file "))
        .flat_map(str::split_whitespace)
        .map(ToString::to_string);
    let watched_files = NIX_DIRENV_WATCHED_FILES
        .iter()
        .map(ToString::to_string)
        .chain(explicitly_watched_files)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter_map(|relative_path| {
            let modified_time = fs::metadata(project_path.join(&relative_path))
                .and_then(|file_metadata| file_metadata.modified())
                .ok()?;
            Some(WatchedFile {
                relative_path,
                modified_time,
            })
        })
        .collect();
    let cache_entries = read_cache_entries(&project_path.join(DIRENV_CACHE_DIRECTORY));
    DirenvSetup::Present(DirenvConfiguration {
        envrc_directives,
        profile_store_paths: cache_entries
            .iter()
            .filter_map(|cache_entry| fs::read_link(cache_entry).ok())
            .map(|store_path| store_path.display().to_string())
            .filter(|store_path| store_path.starts_with("/nix/store/"))
            .collect(),
        exported_variables: cache_entries
            .iter()
            .filter(|cache_entry| {
                cache_entry
                    .extension()
                    .is_some_and(|extension| extension == "rc")
            })
            .filter_map(|rc_path| fs::read_to_string(rc_path).ok())
            .flat_map(|rc_text| exported_variable_names(&rc_text))
            .collect(),
        watched_files,
    })
}

fn read_cache_entries(cache_directory: &Path) -> Vec<PathBuf> {
    let Ok(cache_listing) = fs::read_dir(cache_directory) else {
        return Vec::new();
    };
    cache_listing
        .filter_map(Result::ok)
        .map(|cache_entry| cache_entry.path())
        .collect()
}

fn exported_variable_names(rc_text: &str) -> Vec<String> {
    rc_text
        .match_indices("export ")
        .map(|(export_offset, _)| {
            rc_text[export_offset + "export ".len()..]
                .chars()
                .take_while(|name_character| {
                    name_character.is_ascii_alphanumeric() || *name_character == '_'
                })
                .collect::<String>()
        })
        .filter(|variable_name| !variable_name.is_empty())
        .collect()
}

fn locate_dev_shell_definitions(project_path: &Path) -> Vec<FileLocation> {
    NIX_SHELL_DEFINITIONS
        .iter()
        .map(|definition_name| project_path.join(definition_name))
        .filter_map(|definition_path| {
            let definition_text = fs::read_to_string(&definition_path).ok()?;
            let attribute_offset = Root::parse(&definition_text)
                .syntax()
                .descendants()
                .filter_map(AttrpathValue::cast)
                .find(|attribute_binding| {
                    let attribute_names: Vec<String> = attribute_binding
                        .attrpath()
                        .into_iter()
                        .flat_map(|attribute_path| attribute_path.attrs())
                        .map(|attribute_segment| attribute_segment.syntax().text().to_string())
                        .collect();
                    attribute_names
                        .first()
                        .is_some_and(|attribute_name| attribute_name.starts_with("devShell"))
                })?
                .syntax()
                .text_range()
                .start();
            Some(FileLocation {
                relative_path: relative_display_path(&definition_path, project_path),
                line_number: line_number_at(&definition_text, usize::from(attribute_offset)),
            })
        })
        .collect()
}

fn read_python_environments(
    lookup_current_env: &impl Fn(&str) -> Option<String>,
) -> Vec<PythonEnvironment> {
    PYTHON_ENVIRONMENT_MARKERS
        .iter()
        .filter_map(|environment_marker| {
            let prefix_path = lookup_current_env(environment_marker.marker_variable)?;
            (!prefix_path.is_empty()).then_some(PythonEnvironment {
                environment_kind: environment_marker.environment_kind,
                prefix_path,
            })
        })
        .collect()
}

fn read_cargo_env_entries(project_path: &Path) -> Vec<CargoEnvEntry> {
    project_path
        .ancestors()
        .flat_map(|ancestor_directory| {
            CARGO_CONFIG_FILE_NAMES
                .iter()
                .map(move |config_name| ancestor_directory.join(".cargo").join(config_name))
        })
        .filter(|config_path| config_path.is_file())
        .flat_map(|config_path| {
            let Ok(config_text) = fs::read_to_string(&config_path) else {
                return Vec::new();
            };
            let Ok(config_document) = toml_edit::Document::parse(config_text.clone()) else {
                return Vec::new();
            };
            let Some(env_table) = config_document
                .as_table()
                .get("env")
                .and_then(toml_edit::Item::as_table)
            else {
                return Vec::new();
            };
            env_table
                .iter()
                .filter_map(|env_entry| {
                    let (variable_name, env_item) = env_entry;
                    let key_span = env_table.key(variable_name)?.span()?;
                    let configured_value = env_item
                        .as_str()
                        .or_else(|| env_item.get("value")?.as_str())?
                        .to_string();
                    Some(CargoEnvEntry {
                        variable_name: variable_name.to_string(),
                        configured_value,
                        location: FileLocation {
                            relative_path: relative_display_path(&config_path, project_path),
                            line_number: line_number_at(&config_text, key_span.start),
                        },
                    })
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, os::unix::fs::symlink};

    use tempfile::TempDir;

    use super::*;

    fn read_with_env(project_path: &Path, current_env: &HashMap<&str, &str>) -> ProjectEnvironment {
        read_project_environment(project_path, |variable_name| {
            current_env.get(variable_name).map(ToString::to_string)
        })
    }

    #[test]
    fn reads_direnv_nix_profile_and_watched_files() {
        let project_directory = TempDir::new().unwrap();
        let project_path = project_directory.path();
        fs::write(
            project_path.join(".envrc"),
            "# comment\nwatch_file rust-toolchain.toml\nuse flake\n",
        )
        .unwrap();
        fs::write(project_path.join("rust-toolchain.toml"), "").unwrap();
        fs::write(
            project_path.join("flake.nix"),
            "{\n  # devShells in a comment must not match\n  outputs = _: {\n    \
             devShells.x.default = mkShell {\n      shellHook = ''\n      '';\n    };\n  };\n}\n",
        )
        .unwrap();
        fs::create_dir(project_path.join(".direnv")).unwrap();
        fs::write(
            project_path.join(".direnv/flake-profile-abc.rc"),
            "BINDGEN_EXTRA_CLANG_ARGS='-frandom-seed=f020dci6a9'\nexport \
             BINDGEN_EXTRA_CLANG_ARGS\nexport PATH\n",
        )
        .unwrap();
        symlink(
            "/nix/store/f020dci6a9fianspww2xlh2ri62fyvjw-nix-shell-env",
            project_path.join(".direnv/flake-profile-abc"),
        )
        .unwrap();

        let project_environment = read_with_env(project_path, &HashMap::new());
        let DirenvSetup::Present(direnv_configuration) = &project_environment.direnv_setup else {
            panic!("direnv expected");
        };
        assert_eq!(
            direnv_configuration.envrc_directives,
            vec![
                EnvrcDirective {
                    line_number: 2,
                    directive_text: "watch_file rust-toolchain.toml".to_string()
                },
                EnvrcDirective {
                    line_number: 3,
                    directive_text: "use flake".to_string()
                },
            ]
        );
        assert_eq!(
            direnv_configuration.profile_store_paths,
            vec!["/nix/store/f020dci6a9fianspww2xlh2ri62fyvjw-nix-shell-env"]
        );
        assert!(
            direnv_configuration
                .exported_variables
                .contains("BINDGEN_EXTRA_CLANG_ARGS"),
            "export scanned"
        );
        assert_eq!(
            direnv_configuration
                .watched_files
                .iter()
                .map(|watched_file| watched_file.relative_path.as_str())
                .collect::<Vec<_>>(),
            vec!["flake.nix", "rust-toolchain.toml"]
        );
        assert_eq!(project_environment.dev_shell_locations[0].line_number, 4);
    }

    #[test]
    fn missing_envrc_means_no_direnv() {
        let project_directory = TempDir::new().unwrap();
        assert!(
            matches!(
                read_with_env(project_directory.path(), &HashMap::new()).direnv_setup,
                DirenvSetup::Absent
            ),
            "no .envrc"
        );
    }

    #[test]
    fn detects_python_environments_and_version_managers() {
        let project_directory = TempDir::new().unwrap();
        fs::write(project_directory.path().join("mise.toml"), "").unwrap();
        let project_environment = read_with_env(
            project_directory.path(),
            &HashMap::from([("VIRTUAL_ENV", "/home/user/.venv"), ("CONDA_PREFIX", "")]),
        );
        assert_eq!(
            project_environment.python_environments,
            vec![PythonEnvironment {
                environment_kind: PythonEnvironmentKind::VirtualEnv,
                prefix_path: "/home/user/.venv".to_string()
            }]
        );
        assert_eq!(project_environment.version_managers[0].manager_name, "mise");
    }

    #[test]
    fn locates_cargo_env_entries() {
        let project_directory = TempDir::new().unwrap();
        fs::create_dir(project_directory.path().join(".cargo")).unwrap();
        fs::write(
            project_directory.path().join(".cargo/config.toml"),
            "[alias]\nxtask = \"run\"\n\n[env]\nPYO3_PYTHON = \"/usr/bin/python3\"\n",
        )
        .unwrap();
        assert_eq!(
            read_with_env(project_directory.path(), &HashMap::new()).cargo_env_entries[0],
            CargoEnvEntry {
                variable_name: "PYO3_PYTHON".to_string(),
                configured_value: "/usr/bin/python3".to_string(),
                location: FileLocation {
                    relative_path: ".cargo/config.toml".to_string(),
                    line_number: 5
                }
            }
        );
    }
}
