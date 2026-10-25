use core::error::Error;
use std::{
    env,
    fmt::{Display, Formatter, Result as FmtResult},
    fs, io,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};

use tempfile::TempDir;

struct EnvOverride {
    variable_name: &'static str,
    value: &'static str,
}

struct CargoStep {
    cargo_arguments: &'static [&'static str],
    env_overrides: &'static [EnvOverride],
}

struct ShowcaseScenario {
    scenario_name: &'static str,
    setup_steps: &'static [CargoStep],
    inspect_env_overrides: &'static [EnvOverride],
}

#[derive(Clone, Copy)]
pub enum SyncMode {
    Rewrite,
    Check,
}

#[derive(Debug)]
pub enum ShowcaseSyncError {
    CommandSpawnFailed {
        io_error: io::Error,
    },
    CommandExited {
        exit_status: ExitStatus,
    },
    FileUnreadable {
        file_path: PathBuf,
        io_error: io::Error,
    },
    FileUnwritable {
        file_path: PathBuf,
        io_error: io::Error,
    },
    OpeningMarkerMissing {
        scenario_name: &'static str,
    },
    ClosingMarkerMissing {
        scenario_name: &'static str,
    },
    ReadmesStale {
        stale_readmes: Vec<PathBuf>,
    },
}

impl Display for ShowcaseSyncError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::CommandSpawnFailed { io_error } => {
                write!(formatter, "could not start cargo: {io_error}")
            }
            Self::CommandExited { exit_status } => {
                write!(formatter, "a cargo step failed with {exit_status}")
            }
            Self::FileUnreadable {
                file_path,
                io_error,
            } => {
                write!(
                    formatter,
                    "could not read {}: {io_error}",
                    file_path.display()
                )
            }
            Self::FileUnwritable {
                file_path,
                io_error,
            } => {
                write!(
                    formatter,
                    "could not write {}: {io_error}",
                    file_path.display()
                )
            }
            Self::OpeningMarkerMissing { scenario_name } => {
                write!(
                    formatter,
                    "README has no `<!-- showcase:{scenario_name} -->` marker"
                )
            }
            Self::ClosingMarkerMissing { scenario_name } => {
                write!(
                    formatter,
                    "the {scenario_name} block has no `{CLOSING_MARKER}` marker"
                )
            }
            Self::ReadmesStale { stale_readmes } => {
                let stale_list = stale_readmes
                    .iter()
                    .map(|readme_path| readme_path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(
                    formatter,
                    "stale showcase output in {stale_list}; run `cargo xtask sync-showcase`"
                )
            }
        }
    }
}

impl Error for ShowcaseSyncError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::CommandSpawnFailed { io_error }
            | Self::FileUnreadable { io_error, .. }
            | Self::FileUnwritable { io_error, .. } => Some(io_error),
            Self::CommandExited { .. }
            | Self::OpeningMarkerMissing { .. }
            | Self::ClosingMarkerMissing { .. }
            | Self::ReadmesStale { .. } => None,
        }
    }
}

const CLOSING_MARKER: &str = "<!-- /showcase -->";
const CURRENT_PATH_PLACEHOLDER: &str = "{PATH}";
const ROOT_README: &str = "README.md";
const SKIPPED_COPY_ENTRIES: &[&str] = &["target", "Cargo.lock"];

const SHOWCASE_SCENARIOS: &[ShowcaseScenario] = &[
    ShowcaseScenario {
        scenario_name: "nix-random-seed-drift",
        setup_steps: &[CargoStep {
            cargo_arguments: &["check"],
            env_overrides: &[EnvOverride {
                variable_name: "BINDGEN_EXTRA_CLANG_ARGS",
                value: "-isystem /nix/store/m38wglsqnh1mrmj2cmqa3pjb6w4yjxdi-glibc-2.44-25-dev/\
                        include -frandom-seed=p11zjgl8gg",
            }],
        }],
        inspect_env_overrides: &[EnvOverride {
            variable_name: "BINDGEN_EXTRA_CLANG_ARGS",
            value: "-isystem /nix/store/m38wglsqnh1mrmj2cmqa3pjb6w4yjxdi-glibc-2.44-25-dev/\
                    include -frandom-seed=f020dci6a9",
        }],
    },
    ShowcaseScenario {
        scenario_name: "package-selection-features",
        setup_steps: &[
            CargoStep {
                cargo_arguments: &["check", "-p", "beta"],
                env_overrides: &[],
            },
            CargoStep {
                cargo_arguments: &["check", "-p", "alpha"],
                env_overrides: &[],
            },
        ],
        inspect_env_overrides: &[],
    },
    ShowcaseScenario {
        scenario_name: "volatile-path-watch",
        setup_steps: &[CargoStep {
            cargo_arguments: &["check"],
            env_overrides: &[],
        }],
        inspect_env_overrides: &[EnvOverride {
            variable_name: "PATH",
            value: "/home/user/.venv/bin:{PATH}",
        }],
    },
];

pub fn sync_showcase_readmes(sync_mode: SyncMode) -> Result<(), ShowcaseSyncError> {
    let xtask_directory = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = xtask_directory.parent().unwrap_or(xtask_directory);
    let stale_readmes = SHOWCASE_SCENARIOS
        .iter()
        .map(|showcase_scenario| {
            let frequent_output = run_showcase_scenario(workspace_root, showcase_scenario)?;
            let scenario_readme = workspace_root
                .join("showcase")
                .join(showcase_scenario.scenario_name)
                .join("README.md");
            let scenario_readme_stale = sync_readme_block(
                &scenario_readme,
                showcase_scenario.scenario_name,
                &frequent_output,
                sync_mode,
            )?;
            let root_readme = workspace_root.join(ROOT_README);
            let root_readme_stale =
                has_opening_marker(&root_readme, showcase_scenario.scenario_name)?
                    && sync_readme_block(
                        &root_readme,
                        showcase_scenario.scenario_name,
                        &frequent_output,
                        sync_mode,
                    )?;
            Ok([
                scenario_readme_stale.then_some(scenario_readme),
                root_readme_stale.then_some(root_readme),
            ])
        })
        .collect::<Result<Vec<_>, ShowcaseSyncError>>()?
        .into_iter()
        .flatten()
        .flatten()
        .collect::<Vec<_>>();
    if stale_readmes.is_empty() {
        return Ok(());
    }
    Err(ShowcaseSyncError::ReadmesStale { stale_readmes })
}

fn run_showcase_scenario(
    workspace_root: &Path,
    showcase_scenario: &ShowcaseScenario,
) -> Result<String, ShowcaseSyncError> {
    let scenario_directory =
        TempDir::new().map_err(|io_error| ShowcaseSyncError::CommandSpawnFailed { io_error })?;
    copy_directory(
        &workspace_root
            .join("showcase")
            .join(showcase_scenario.scenario_name),
        scenario_directory.path(),
    )?;
    showcase_scenario
        .setup_steps
        .iter()
        .try_for_each(|cargo_step| {
            let mut cargo_command = Command::new("cargo");
            cargo_command
                .args(cargo_step.cargo_arguments)
                .arg("--quiet")
                .current_dir(scenario_directory.path());
            apply_env_overrides(&mut cargo_command, cargo_step.env_overrides);
            run_to_completion(&mut cargo_command).map(drop)
        })?;
    let mut frequent_command = Command::new("cargo");
    frequent_command
        .args([
            "run",
            "--quiet",
            "--package",
            "cargo-frequent",
            "--",
            "--path",
        ])
        .arg(scenario_directory.path())
        .current_dir(workspace_root);
    apply_env_overrides(
        &mut frequent_command,
        showcase_scenario.inspect_env_overrides,
    );
    run_to_completion(&mut frequent_command)
}

fn run_to_completion(command: &mut Command) -> Result<String, ShowcaseSyncError> {
    let command_output = command
        .output()
        .map_err(|io_error| ShowcaseSyncError::CommandSpawnFailed { io_error })?;
    if !command_output.status.success() {
        eprint!("{}", String::from_utf8_lossy(&command_output.stderr));
        return Err(ShowcaseSyncError::CommandExited {
            exit_status: command_output.status,
        });
    }
    Ok(String::from_utf8_lossy(&command_output.stdout).into_owned())
}

fn apply_env_overrides(command: &mut Command, env_overrides: &[EnvOverride]) {
    let current_path = env::var("PATH").unwrap_or_default();
    command
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET_DIR");
    env_overrides.iter().for_each(|env_override| {
        command.env(
            env_override.variable_name,
            env_override
                .value
                .replace(CURRENT_PATH_PLACEHOLDER, &current_path),
        );
    });
}

fn copy_directory(
    source_directory: &Path,
    destination_directory: &Path,
) -> Result<(), ShowcaseSyncError> {
    let unreadable = |io_error| ShowcaseSyncError::FileUnreadable {
        file_path: source_directory.to_path_buf(),
        io_error,
    };
    fs::create_dir_all(destination_directory).map_err(|io_error| {
        ShowcaseSyncError::FileUnwritable {
            file_path: destination_directory.to_path_buf(),
            io_error,
        }
    })?;
    fs::read_dir(source_directory)
        .map_err(unreadable)?
        .filter_map(Result::ok)
        .filter(|directory_entry| {
            !SKIPPED_COPY_ENTRIES
                .iter()
                .any(|skipped_entry| directory_entry.file_name() == *skipped_entry)
        })
        .try_for_each(|directory_entry| {
            let source_path = directory_entry.path();
            let destination_path = destination_directory.join(directory_entry.file_name());
            if source_path.is_dir() {
                return copy_directory(&source_path, &destination_path);
            }
            fs::copy(&source_path, &destination_path)
                .map(drop)
                .map_err(|io_error| ShowcaseSyncError::FileUnwritable {
                    file_path: destination_path,
                    io_error,
                })
        })
}

fn has_opening_marker(
    readme_path: &Path,
    scenario_name: &'static str,
) -> Result<bool, ShowcaseSyncError> {
    let readme_text =
        fs::read_to_string(readme_path).map_err(|io_error| ShowcaseSyncError::FileUnreadable {
            file_path: readme_path.to_path_buf(),
            io_error,
        })?;
    Ok(readme_text.contains(&format!("<!-- showcase:{scenario_name} -->")))
}

fn sync_readme_block(
    readme_path: &Path,
    scenario_name: &'static str,
    frequent_output: &str,
    sync_mode: SyncMode,
) -> Result<bool, ShowcaseSyncError> {
    let readme_text =
        fs::read_to_string(readme_path).map_err(|io_error| ShowcaseSyncError::FileUnreadable {
            file_path: readme_path.to_path_buf(),
            io_error,
        })?;
    let opening_marker = format!("<!-- showcase:{scenario_name} -->");
    let Some(block_start) = readme_text.find(&opening_marker) else {
        return Err(ShowcaseSyncError::OpeningMarkerMissing { scenario_name });
    };
    let content_start = block_start + opening_marker.len();
    let Some(content_length) = readme_text[content_start..].find(CLOSING_MARKER) else {
        return Err(ShowcaseSyncError::ClosingMarkerMissing { scenario_name });
    };
    let synced_text = format!(
        "{}\n```text\n{}\n```\n{}",
        &readme_text[..content_start],
        frequent_output.trim_end(),
        &readme_text[content_start + content_length..]
    );
    if synced_text == readme_text {
        return Ok(false);
    }
    match sync_mode {
        SyncMode::Check => Ok(true),
        SyncMode::Rewrite => {
            fs::write(readme_path, synced_text).map_err(|io_error| {
                ShowcaseSyncError::FileUnwritable {
                    file_path: readme_path.to_path_buf(),
                    io_error,
                }
            })?;
            Ok(false)
        }
    }
}
