use std::time::SystemTime;

use crate::{
    target_reading::{
        CodeLocation, FileLocation,
        cargo_run_variables::is_cargo_run_variable,
        environment_sources::{
            CargoEnvEntry, DirenvConfiguration, DirenvSetup, ProjectEnvironment, PythonEnvironment,
            PythonEnvironmentKind, VersionManager,
        },
    },
    trigger_detection::{
        ChainLevel, SourceStep,
        env_value_change::EnvValueChange,
        fix_hints::{
            CARGO_ENV_DOCUMENTATION, DocumentationLink, FixHint, NIX_RANDOM_SEED_DOCUMENTATION,
            PYO3_PYTHON_DOCUMENTATION, is_nix_store_update,
        },
    },
};

const NIX_STORE_PREFIX: &str = "/nix/store/";
const SEED_HASH_LENGTH: usize = 10;
const SEED_FLAG: &str = "-frandom-seed=";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviousRun {
    RecordedAt(SystemTime),
    Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvAttribution {
    pub cause_location: CodeLocation,
    pub chain_levels: Vec<ChainLevel>,
    pub fix_hints: Vec<FixHint>,
}

pub struct EnvChange<'change> {
    pub variable_name: &'change str,
    pub value_change: &'change EnvValueChange,
    pub previous_run: PreviousRun,
}

#[must_use]
pub fn attribute_env_change(
    env_change: &EnvChange<'_>,
    project_environment: &ProjectEnvironment,
) -> EnvAttribution {
    if is_cargo_run_variable(env_change.variable_name) {
        return attribute_to_nested_cargo(env_change.variable_name);
    }
    let changed_tokens = env_change.value_change.changed_tokens();
    if let Some(cargo_env_entry) = project_environment
        .cargo_env_entries
        .iter()
        .find(|cargo_env_entry| cargo_env_entry.variable_name == env_change.variable_name)
    {
        return attribute_to_cargo_config(env_change.variable_name, cargo_env_entry);
    }
    if let Some(python_environment) =
        project_environment
            .python_environments
            .iter()
            .find(|python_environment| {
                changed_tokens
                    .iter()
                    .any(|changed_token| changed_token.starts_with(&python_environment.prefix_path))
            })
    {
        return attribute_to_python_environment(
            python_environment,
            &project_environment.direnv_setup,
        );
    }
    if let Some(version_manager) =
        project_environment
            .version_managers
            .iter()
            .find(|version_manager| {
                changed_tokens
                    .iter()
                    .any(|changed_token| changed_token.contains(version_manager.path_fragment))
            })
    {
        return attribute_to_version_manager(version_manager);
    }
    match &project_environment.direnv_setup {
        DirenvSetup::Present(direnv_configuration)
            if direnv_configuration
                .exported_variables
                .contains(env_change.variable_name) =>
        {
            attribute_to_direnv(env_change, direnv_configuration, project_environment)
        }
        DirenvSetup::Present(_) => attribute_outside_direnv(env_change.variable_name),
        DirenvSetup::Absent => EnvAttribution::default(),
    }
}

fn attribute_to_nested_cargo(variable_name: &str) -> EnvAttribution {
    EnvAttribution {
        cause_location: CodeLocation::Nowhere,
        chain_levels: vec![ChainLevel {
            level_steps: vec![SourceStep {
                location: CodeLocation::Nowhere,
                step_label: "inherited",
                step_text: format!(
                    "the last build ran inside `cargo run` or `cargo xtask`\nwhich sets \
                     {variable_name} for the program it starts"
                ),
            }],
        }],
        fix_hints: vec![FixHint {
            documentation_link: DocumentationLink::NoLink,
            location: CodeLocation::Nowhere,
            summary: format!(
                "terminal builds lack {variable_name}, so they rerun the build script"
            ),
            suggestion: format!(
                "remove the cargo-run variables before the xtask spawns \
                 cargo:\nCommand::new(\"cargo\").env_remove(\"{variable_name}\")"
            ),
        }],
    }
}

fn attribute_to_cargo_config(
    variable_name: &str,
    cargo_env_entry: &CargoEnvEntry,
) -> EnvAttribution {
    let entry_location = CodeLocation::Line(cargo_env_entry.location.clone());
    EnvAttribution {
        cause_location: entry_location.clone(),
        chain_levels: vec![ChainLevel {
            level_steps: vec![SourceStep {
                location: entry_location.clone(),
                step_label: "cargo [env]",
                step_text: format!("{variable_name} = \"{}\"", cargo_env_entry.configured_value),
            }],
        }],
        fix_hints: vec![FixHint {
            documentation_link: CARGO_ENV_DOCUMENTATION,
            location: entry_location,
            summary: format!("{variable_name} comes from cargo [env] and its value changed"),
            suggestion: format!(
                "one rebuild is expected after editing it\nif it keeps flipping, another tool \
                 passes a different value\n(environment variable or --config \
                 'env.{variable_name}=…'): make it use the same one"
            ),
        }],
    }
}

fn attribute_to_python_environment(
    python_environment: &PythonEnvironment,
    direnv_setup: &DirenvSetup,
) -> EnvAttribution {
    let prefix_path = &python_environment.prefix_path;
    let environment_name = match python_environment.environment_kind {
        PythonEnvironmentKind::VirtualEnv => "Python virtualenv",
        PythonEnvironmentKind::Conda => "conda environment",
    };
    let (pinning_location, pinning_suggestion) = match direnv_setup {
        DirenvSetup::Present(_) => (
            CodeLocation::File {
                file_path: ".envrc".to_string(),
            },
            format!("add `export PYO3_PYTHON={prefix_path}/bin/python`"),
        ),
        DirenvSetup::Absent => (
            CodeLocation::File {
                file_path: ".cargo/config.toml".to_string(),
            },
            format!("add `PYO3_PYTHON = \"{prefix_path}/bin/python\"` under [env]"),
        ),
    };
    EnvAttribution {
        cause_location: CodeLocation::Nowhere,
        chain_levels: vec![ChainLevel {
            level_steps: vec![SourceStep {
                location: CodeLocation::Nowhere,
                step_label: "activated",
                step_text: format!(
                    "{environment_name} {prefix_path}\nactivation prepends {prefix_path}/bin to \
                     PATH"
                ),
            }],
        }],
        fix_hints: vec![FixHint {
            documentation_link: PYO3_PYTHON_DOCUMENTATION,
            location: pinning_location,
            summary: format!(
                "activating the {environment_name} changes PATH, which the build script watches"
            ),
            suggestion: format!(
                "pin the interpreter so the build script stops searching \
                 PATH:\n{pinning_suggestion}"
            ),
        }],
    }
}

fn attribute_to_version_manager(version_manager: &VersionManager) -> EnvAttribution {
    let manager_name = version_manager.manager_name;
    let config_location = CodeLocation::File {
        file_path: version_manager.config_file.clone(),
    };
    EnvAttribution {
        cause_location: config_location.clone(),
        chain_levels: vec![ChainLevel {
            level_steps: vec![SourceStep {
                location: config_location.clone(),
                step_label: "manager",
                step_text: format!("{manager_name} puts the selected tool versions on PATH"),
            }],
        }],
        fix_hints: vec![FixHint {
            documentation_link: DocumentationLink::NoLink,
            location: config_location,
            summary: format!("the {manager_name} tool version on PATH changed"),
            suggestion: format!(
                "one rebuild is expected after editing it\nif it keeps flipping, activate \
                 {manager_name} in every shell that runs cargo,\nor point the build script at the \
                 tool path directly"
            ),
        }],
    }
}

fn attribute_outside_direnv(variable_name: &str) -> EnvAttribution {
    let envrc_location = CodeLocation::File {
        file_path: ".envrc".to_string(),
    };
    EnvAttribution {
        cause_location: CodeLocation::Nowhere,
        chain_levels: vec![ChainLevel {
            level_steps: vec![SourceStep {
                location: envrc_location,
                step_label: "origin",
                step_text: format!(
                    "does not export {variable_name}\nso the shell or tool that ran cargo set it"
                ),
            }],
        }],
        fix_hints: vec![FixHint {
            documentation_link: DocumentationLink::NoLink,
            location: CodeLocation::Nowhere,
            summary: format!(
                "{variable_name} differs between the direnv shell and another environment"
            ),
            suggestion: "run cargo only from the direnv shell (editors and agents may start it \
                         without),\nor give the other tool its own CARGO_TARGET_DIR"
                .to_string(),
        }],
    }
}

fn attribute_to_direnv(
    env_change: &EnvChange<'_>,
    direnv_configuration: &DirenvConfiguration,
    project_environment: &ProjectEnvironment,
) -> EnvAttribution {
    let changed_tokens = env_change.value_change.changed_tokens();
    let envrc_step = locate_envrc_step(direnv_configuration);
    let dev_shell_location = project_environment
        .dev_shell_locations
        .first()
        .cloned()
        .map_or(CodeLocation::Nowhere, CodeLocation::Line);
    let seed_flag_changed = changed_tokens
        .iter()
        .any(|changed_token| changed_token.starts_with(SEED_FLAG));
    let matching_store_path = match_profile_store_path(direnv_configuration, &changed_tokens);
    let changed_watched_files: Vec<&str> = match env_change.previous_run {
        PreviousRun::RecordedAt(previous_run_time) => direnv_configuration
            .watched_files
            .iter()
            .filter(|watched_file| watched_file.modified_time > previous_run_time)
            .map(|watched_file| watched_file.relative_path.as_str())
            .collect(),
        PreviousRun::Unknown => Vec::new(),
    };

    let dev_shell_step = matching_store_path
        .as_ref()
        .map(|matched_store_path| SourceStep {
            location: dev_shell_location.clone(),
            step_label: "dev shell",
            step_text: if seed_flag_changed {
                format!(
                    "{}\nthe new seed {} is the start of this store hash\n(nixpkgs seeds the \
                     compiler with it, so every change to the shell changes the seed)",
                    matched_store_path.abbreviated_path, matched_store_path.hash_prefix
                )
            } else {
                format!(
                    "{}\nthe new value contains this store hash",
                    matched_store_path.abbreviated_path
                )
            },
        });
    let changed_steps = changed_watched_files
        .iter()
        .map(|changed_file| SourceStep {
            location: CodeLocation::File {
                file_path: (*changed_file).to_string(),
            },
            step_label: "changed",
            step_text: "modified after the build scripts last ran".to_string(),
        })
        .collect();
    let cause_location = match &dev_shell_location {
        CodeLocation::Nowhere => envrc_step.location.clone(),
        CodeLocation::File { .. } | CodeLocation::Line(_) => dev_shell_location.clone(),
    };
    let chain_levels = vec![
        ChainLevel {
            level_steps: vec![envrc_step],
        },
        ChainLevel {
            level_steps: dev_shell_step.into_iter().collect(),
        },
        ChainLevel {
            level_steps: changed_steps,
        },
    ];

    EnvAttribution {
        cause_location,
        chain_levels,
        fix_hints: direnv_fix_hints(&DirenvFindings {
            variable_name: env_change.variable_name,
            value_change: env_change.value_change,
            dev_shell_location,
            seed_flag_changed,
        }),
    }
}

fn locate_envrc_step(direnv_configuration: &DirenvConfiguration) -> SourceStep {
    direnv_configuration
        .envrc_directives
        .iter()
        .find(|envrc_directive| {
            ["use ", "layout ", "dotenv", "source_env"]
                .iter()
                .any(|directive_prefix| {
                    envrc_directive.directive_text.starts_with(directive_prefix)
                })
        })
        .map_or_else(
            || SourceStep {
                location: CodeLocation::File {
                    file_path: ".envrc".to_string(),
                },
                step_label: "direnv",
                step_text: "loads the dev shell".to_string(),
            },
            |envrc_directive| SourceStep {
                location: CodeLocation::Line(FileLocation {
                    relative_path: ".envrc".to_string(),
                    line_number: envrc_directive.line_number,
                }),
                step_label: "direnv",
                step_text: envrc_directive.directive_text.clone(),
            },
        )
}

fn match_profile_store_path(
    direnv_configuration: &DirenvConfiguration,
    changed_tokens: &[&String],
) -> Option<MatchedStorePath> {
    direnv_configuration
        .profile_store_paths
        .iter()
        .find_map(|profile_store_path| {
            let store_entry = profile_store_path.strip_prefix(NIX_STORE_PREFIX)?;
            let hash_prefix = store_entry.get(..SEED_HASH_LENGTH)?;
            let store_name = store_entry
                .split_once('-')
                .map_or("", |store_parts| store_parts.1);
            changed_tokens
                .iter()
                .any(|changed_token| changed_token.contains(hash_prefix))
                .then(|| MatchedStorePath {
                    hash_prefix: hash_prefix.to_string(),
                    abbreviated_path: format!("{NIX_STORE_PREFIX}{hash_prefix}…-{store_name}"),
                })
        })
}

struct MatchedStorePath {
    hash_prefix: String,
    abbreviated_path: String,
}

struct DirenvFindings<'finding> {
    variable_name: &'finding str,
    value_change: &'finding EnvValueChange,
    dev_shell_location: CodeLocation,
    seed_flag_changed: bool,
}

fn direnv_fix_hints(direnv_findings: &DirenvFindings<'_>) -> Vec<FixHint> {
    let variable_name = direnv_findings.variable_name;
    if direnv_findings.seed_flag_changed {
        return vec![FixHint {
            documentation_link: NIX_RANDOM_SEED_DOCUMENTATION,
            location: direnv_findings.dev_shell_location.clone(),
            summary: "pin the seed in the dev shell:".to_string(),
            suggestion: "env.NIX_OUTPATH_USED_AS_RANDOM_SEED = \"dev-shell\";\n(nixpkgs' \
                         reproducible-builds hook then seeds with \"dev-shell\" instead of the \
                         store hash)"
                .to_string(),
        }];
    }
    if is_nix_store_update(direnv_findings.value_change) {
        return vec![FixHint {
            documentation_link: DocumentationLink::NoLink,
            location: CodeLocation::Nowhere,
            summary: "one-off: the dev shell's Nix store paths moved".to_string(),
            suggestion: "the next build reruns these build scripts once, then they are stable \
                         again"
                .to_string(),
        }];
    }
    vec![FixHint {
        documentation_link: DocumentationLink::NoLink,
        location: direnv_findings.dev_shell_location.clone(),
        summary: format!("{variable_name} comes from the dev shell that direnv loads"),
        suggestion: format!(
            "if it keeps changing, give it a fixed value in the dev shell:\nenv.{variable_name} = \
             \"…\";"
        ),
    }]
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, time::Duration};

    use super::*;

    fn attributed_steps(env_attribution: &EnvAttribution) -> Vec<SourceStep> {
        env_attribution
            .chain_levels
            .iter()
            .flat_map(|chain_level| chain_level.level_steps.clone())
            .collect()
    }
    use crate::target_reading::environment_sources::{EnvrcDirective, WatchedFile};

    fn nix_project(watched_file_modified: SystemTime) -> ProjectEnvironment {
        ProjectEnvironment {
            direnv_setup: DirenvSetup::Present(DirenvConfiguration {
                envrc_directives: vec![
                    EnvrcDirective {
                        line_number: 1,
                        directive_text: "watch_file rust-toolchain.toml".to_string(),
                    },
                    EnvrcDirective {
                        line_number: 2,
                        directive_text: "use flake".to_string(),
                    },
                ],
                profile_store_paths: vec![
                    "/nix/store/f020dci6a9fianspww2xlh2ri62fyvjw-nix-shell-env".to_string(),
                ],
                exported_variables: BTreeSet::from([
                    "BINDGEN_EXTRA_CLANG_ARGS".to_string(),
                    "PATH".to_string(),
                ]),
                watched_files: vec![WatchedFile {
                    relative_path: "flake.nix".to_string(),
                    modified_time: watched_file_modified,
                }],
            }),
            dev_shell_locations: vec![FileLocation {
                relative_path: "flake.nix".to_string(),
                line_number: 237,
            }],
            python_environments: Vec::new(),
            version_managers: Vec::new(),
            cargo_env_entries: Vec::new(),
        }
    }

    #[test]
    fn traces_random_seed_to_dev_shell_and_flake_edit() {
        let previous_run_time = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let seed_change = EnvValueChange::between(
            "-O2 -frandom-seed=p11zjgl8gg",
            "-O2 -frandom-seed=f020dci6a9",
        )
        .unwrap();
        let env_attribution = attribute_env_change(
            &EnvChange {
                variable_name: "BINDGEN_EXTRA_CLANG_ARGS",
                value_change: &seed_change,
                previous_run: PreviousRun::RecordedAt(previous_run_time),
            },
            &nix_project(previous_run_time + Duration::from_secs(60)),
        );
        assert_eq!(
            attributed_steps(&env_attribution),
            vec![
                SourceStep {
                    location: CodeLocation::Line(FileLocation {
                        relative_path: ".envrc".to_string(),
                        line_number: 2,
                    }),
                    step_label: "direnv",
                    step_text: "use flake".to_string()
                },
                SourceStep {
                    location: CodeLocation::Line(FileLocation {
                        relative_path: "flake.nix".to_string(),
                        line_number: 237,
                    }),
                    step_label: "dev shell",
                    step_text: "/nix/store/f020dci6a9…-nix-shell-env\nthe new seed f020dci6a9 is \
                                the start of this store hash\n(nixpkgs seeds the compiler with \
                                it, so every change to the shell changes the seed)"
                        .to_string()
                },
                SourceStep {
                    location: CodeLocation::File {
                        file_path: "flake.nix".to_string(),
                    },
                    step_label: "changed",
                    step_text: "modified after the build scripts last ran".to_string()
                },
            ]
        );
        assert_eq!(
            env_attribution.fix_hints[0].summary,
            "pin the seed in the dev shell:"
        );
        assert_eq!(
            env_attribution.fix_hints[0].location.to_string(),
            "flake.nix:237"
        );
        assert_eq!(env_attribution.cause_location.to_string(), "flake.nix:237");
        assert!(
            env_attribution.fix_hints[0]
                .suggestion
                .starts_with("env.NIX_OUTPATH_USED_AS_RANDOM_SEED = \"dev-shell\";"),
            "declarative pin"
        );
        assert_eq!(
            env_attribution.fix_hints.len(),
            1,
            "no shellHook workaround"
        );
    }

    #[test]
    fn older_watched_files_are_not_blamed() {
        let previous_run_time = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let seed_change =
            EnvValueChange::between("-frandom-seed=p11zjgl8gg", "-frandom-seed=f020dci6a9")
                .unwrap();
        let env_attribution = attribute_env_change(
            &EnvChange {
                variable_name: "BINDGEN_EXTRA_CLANG_ARGS",
                value_change: &seed_change,
                previous_run: PreviousRun::RecordedAt(previous_run_time),
            },
            &nix_project(SystemTime::UNIX_EPOCH),
        );
        assert_eq!(
            attributed_steps(&env_attribution)
                .iter()
                .map(|source_step| source_step.step_label)
                .collect::<Vec<_>>(),
            vec!["direnv", "dev shell"],
            "no changed step"
        );
    }

    #[test]
    fn blames_virtualenv_for_path_change() {
        let mut project_environment = nix_project(SystemTime::UNIX_EPOCH);
        project_environment.python_environments = vec![PythonEnvironment {
            environment_kind: PythonEnvironmentKind::VirtualEnv,
            prefix_path: "/home/user/.venv".to_string(),
        }];
        let path_change =
            EnvValueChange::between("/usr/bin", "/home/user/.venv/bin:/usr/bin").unwrap();
        let env_attribution = attribute_env_change(
            &EnvChange {
                variable_name: "PATH",
                value_change: &path_change,
                previous_run: PreviousRun::Unknown,
            },
            &project_environment,
        );
        assert_eq!(
            attributed_steps(&env_attribution)[0].step_label,
            "activated"
        );
        assert!(
            env_attribution.fix_hints[0]
                .suggestion
                .contains("export PYO3_PYTHON=/home/user/.venv/bin/python`"),
            "{:?}",
            env_attribution.fix_hints
        );
    }

    #[test]
    fn blames_cargo_config_first() {
        let mut project_environment = nix_project(SystemTime::UNIX_EPOCH);
        project_environment.cargo_env_entries = vec![CargoEnvEntry {
            variable_name: "PYO3_PYTHON".to_string(),
            configured_value: "/opt/python3".to_string(),
            location: FileLocation {
                relative_path: ".cargo/config.toml".to_string(),
                line_number: 5,
            },
        }];
        let python_change = EnvValueChange::between("/usr/bin/python3", "/opt/python3").unwrap();
        let env_attribution = attribute_env_change(
            &EnvChange {
                variable_name: "PYO3_PYTHON",
                value_change: &python_change,
                previous_run: PreviousRun::Unknown,
            },
            &project_environment,
        );
        assert_eq!(
            attributed_steps(&env_attribution)[0].location.to_string(),
            ".cargo/config.toml:5"
        );
    }

    #[test]
    fn notes_variables_not_exported_by_direnv() {
        let cc_change = EnvValueChange::between("gcc", "clang").unwrap();
        let env_attribution = attribute_env_change(
            &EnvChange {
                variable_name: "CC",
                value_change: &cc_change,
                previous_run: PreviousRun::Unknown,
            },
            &nix_project(SystemTime::UNIX_EPOCH),
        );
        assert_eq!(attributed_steps(&env_attribution)[0].step_label, "origin");
    }

    #[test]
    fn explains_variables_inherited_from_cargo_run() {
        let manifest_change = EnvValueChange::unset_from("/home/user/project/xtask");
        let env_attribution = attribute_env_change(
            &EnvChange {
                variable_name: "CARGO_MANIFEST_DIR",
                value_change: &manifest_change,
                previous_run: PreviousRun::Unknown,
            },
            &nix_project(SystemTime::UNIX_EPOCH),
        );
        assert_eq!(
            attributed_steps(&env_attribution)[0].step_label,
            "inherited"
        );
        assert!(
            env_attribution.fix_hints[0]
                .suggestion
                .contains("env_remove"),
            "{:?}",
            env_attribution.fix_hints
        );
    }

    #[test]
    fn projects_without_managers_have_no_attribution() {
        let mut project_environment = nix_project(SystemTime::UNIX_EPOCH);
        project_environment.direnv_setup = DirenvSetup::Absent;
        let cc_change = EnvValueChange::between("gcc", "clang").unwrap();
        assert_eq!(
            attribute_env_change(
                &EnvChange {
                    variable_name: "CC",
                    value_change: &cc_change,
                    previous_run: PreviousRun::Unknown,
                },
                &project_environment,
            ),
            EnvAttribution::default()
        );
    }
}
