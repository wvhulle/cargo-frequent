use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
    process::ExitCode,
    time::SystemTime,
};

use fs_extra::dir::{CopyOptions, copy as copy_directory};
use libtest_mimic::{Arguments, Failed, Trial};
use serde::Deserialize;
use tempfile::TempDir;
use xshell::{Cmd, Shell, cmd};

const E2E_CRATES_DIRECTORY: &str = "e2e-test-crates";
const SCENARIO_FILE: &str = "scenario.toml";
const UPDATE_VARIABLE: &str = "UPDATE_E2E";
const CURRENT_PATH_PLACEHOLDER: &str = "{PATH}";
const CLOSING_MARKER: &str = "<!-- /e2e -->";
const ROOT_README: &str = "README.md";
const COPY_EXCLUDED_ENTRIES: &[&str] = &["target", "Cargo.lock"];
const AMBIENT_VARIABLES: &[&str] = &[
    "CARGO_TARGET_DIR",
    "CARGO_BUILD_TARGET_DIR",
    "VIRTUAL_ENV",
    "CONDA_PREFIX",
    "CONDA_DEFAULT_ENV",
    "DIRENV_FILE",
    "DIRENV_DIR",
    "IN_NIX_SHELL",
];

#[derive(Deserialize)]
struct E2eScenario {
    #[serde(default)]
    isolated_variables: Vec<String>,
    #[serde(rename = "step")]
    setup_steps: Vec<ScenarioStep>,
    inspect: InspectStep,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum ScenarioStep {
    Cargo {
        cargo_arguments: Vec<String>,
        #[serde(default)]
        env_overrides: BTreeMap<String, String>,
    },
    Write {
        file_path: String,
        file_contents: String,
    },
    Symlink {
        link_path: String,
        link_target: String,
    },
    Touch {
        file_path: String,
    },
}

#[derive(Deserialize)]
struct InspectStep {
    #[serde(default)]
    env_overrides: BTreeMap<String, String>,
    #[serde(default)]
    frequent_arguments: Vec<String>,
}

struct ScenarioRun {
    workspace_root: PathBuf,
    crate_directory: PathBuf,
    crate_name: String,
}

fn main() -> ExitCode {
    let test_arguments = Arguments::from_args();
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let Ok(crate_listing) = fs::read_dir(workspace_root.join(E2E_CRATES_DIRECTORY)) else {
        eprintln!("no {E2E_CRATES_DIRECTORY} directory");
        return ExitCode::FAILURE;
    };
    let mut crate_directories: Vec<PathBuf> = crate_listing
        .filter_map(Result::ok)
        .map(|directory_entry| directory_entry.path())
        .filter(|crate_directory| crate_directory.join(SCENARIO_FILE).is_file())
        .collect();
    crate_directories.sort();
    let trials = crate_directories
        .into_iter()
        .map(|crate_directory| {
            let scenario_run = ScenarioRun {
                crate_name: crate_directory
                    .file_name()
                    .map(|directory_name| directory_name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                workspace_root: workspace_root.clone(),
                crate_directory,
            };
            Trial::test(scenario_run.crate_name.clone(), move || {
                run_scenario(&scenario_run)
            })
        })
        .collect();
    libtest_mimic::run(&test_arguments, trials).exit_code()
}

fn run_scenario(scenario_run: &ScenarioRun) -> Result<(), Failed> {
    let scenario_text = fs::read_to_string(scenario_run.crate_directory.join(SCENARIO_FILE))?;
    let e2e_scenario: E2eScenario = toml::from_str(&scenario_text)?;
    let scenario_directory = TempDir::new()?;
    let scenario_root = scenario_directory.path();
    copy_directory(
        &scenario_run.crate_directory,
        scenario_root,
        &CopyOptions::new().content_only(true),
    )?;
    COPY_EXCLUDED_ENTRIES
        .iter()
        .map(|excluded_entry| scenario_root.join(excluded_entry))
        .filter(|excluded_path| excluded_path.exists())
        .try_for_each(|excluded_path| {
            if excluded_path.is_dir() {
                fs::remove_dir_all(excluded_path)
            } else {
                fs::remove_file(excluded_path)
            }
        })?;

    let scenario_shell = Shell::new()?;
    scenario_shell.change_dir(scenario_root);
    e2e_scenario
        .setup_steps
        .iter()
        .try_for_each(|scenario_step| {
            run_step(&scenario_shell, scenario_root, &e2e_scenario, scenario_step)
        })?;

    let snapshot_before_inspection = snapshot_directory(scenario_root)?;
    let frequent_binary = env!("CARGO_BIN_EXE_cargo-frequent");
    let frequent_arguments = &e2e_scenario.inspect.frequent_arguments;
    let frequent_command = cmd!(
        scenario_shell,
        "{frequent_binary} --color never --path {scenario_root} {frequent_arguments...}"
    );
    let frequent_output = isolate_environment(
        frequent_command,
        &e2e_scenario,
        &e2e_scenario.inspect.env_overrides,
    )
    .quiet()
    .read()?;

    let snapshot_after_inspection = snapshot_directory(scenario_root)?;
    let changed_entries: Vec<&FileSnapshot> = snapshot_before_inspection
        .symmetric_difference(&snapshot_after_inspection)
        .collect();
    if !changed_entries.is_empty() {
        return Err(
            format!("cargo frequent changed the inspected crate: {changed_entries:?}").into(),
        );
    }

    let crate_readme = scenario_run.crate_directory.join("README.md");
    let root_readme = scenario_run.workspace_root.join(ROOT_README);
    let update_requested = env::var_os(UPDATE_VARIABLE).is_some();
    let marked_readmes: Vec<&PathBuf> = [&crate_readme, &root_readme]
        .into_iter()
        .filter(|readme_path| {
            fs::read_to_string(readme_path).is_ok_and(|readme_text| {
                readme_text.contains(&opening_marker(&scenario_run.crate_name))
            })
        })
        .collect();
    if marked_readmes.is_empty() {
        return Err(format!(
            "no README contains {}",
            opening_marker(&scenario_run.crate_name)
        )
        .into());
    }
    marked_readmes.into_iter().try_for_each(|readme_path| {
        sync_readme_block(
            readme_path,
            &scenario_run.crate_name,
            &frequent_output,
            update_requested,
        )
    })
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct FileSnapshot {
    relative_path: PathBuf,
    byte_length: u64,
    modified_time: SystemTime,
}

fn snapshot_directory(snapshot_root: &Path) -> Result<BTreeSet<FileSnapshot>, Failed> {
    let mut file_snapshots = BTreeSet::new();
    collect_file_snapshots(snapshot_root, snapshot_root, &mut file_snapshots)?;
    Ok(file_snapshots)
}

fn collect_file_snapshots(
    current_directory: &Path,
    snapshot_root: &Path,
    file_snapshots: &mut BTreeSet<FileSnapshot>,
) -> Result<(), Failed> {
    fs::read_dir(current_directory)?
        .filter_map(Result::ok)
        .try_for_each(|directory_entry| {
            let entry_path = directory_entry.path();
            let entry_metadata = fs::symlink_metadata(&entry_path)?;
            file_snapshots.insert(FileSnapshot {
                relative_path: entry_path.strip_prefix(snapshot_root)?.to_path_buf(),
                byte_length: entry_metadata.len(),
                modified_time: entry_metadata.modified()?,
            });
            if entry_metadata.is_dir() {
                collect_file_snapshots(&entry_path, snapshot_root, file_snapshots)?;
            }
            Ok(())
        })
}

fn run_step(
    scenario_shell: &Shell,
    scenario_root: &Path,
    e2e_scenario: &E2eScenario,
    scenario_step: &ScenarioStep,
) -> Result<(), Failed> {
    match scenario_step {
        ScenarioStep::Cargo {
            cargo_arguments,
            env_overrides,
        } => {
            let cargo_command = cmd!(scenario_shell, "cargo {cargo_arguments...} --quiet");
            isolate_environment(cargo_command, e2e_scenario, env_overrides)
                .quiet()
                .run()?;
        }
        ScenarioStep::Write {
            file_path,
            file_contents,
        } => {
            let written_path = scenario_root.join(file_path);
            if let Some(parent_directory) = written_path.parent() {
                fs::create_dir_all(parent_directory)?;
            }
            fs::write(written_path, file_contents)?;
        }
        ScenarioStep::Symlink {
            link_path,
            link_target,
        } => {
            let created_link = scenario_root.join(link_path);
            if let Some(parent_directory) = created_link.parent() {
                fs::create_dir_all(parent_directory)?;
            }
            symlink(link_target, created_link)?;
        }
        ScenarioStep::Touch { file_path } => {
            fs::File::options()
                .append(true)
                .open(scenario_root.join(file_path))?
                .set_modified(SystemTime::now())?;
        }
    }
    Ok(())
}

fn isolate_environment<'shell>(
    scenario_command: Cmd<'shell>,
    e2e_scenario: &E2eScenario,
    env_overrides: &BTreeMap<String, String>,
) -> Cmd<'shell> {
    let current_path = env::var("PATH").unwrap_or_default();
    let cleared_command = AMBIENT_VARIABLES
        .iter()
        .map(ToString::to_string)
        .chain(e2e_scenario.isolated_variables.iter().cloned())
        .fold(scenario_command, |command_so_far, cleared_variable| {
            command_so_far.env_remove(cleared_variable)
        });
    env_overrides
        .iter()
        .fold(cleared_command, |command_so_far, env_override| {
            let (variable_name, variable_value) = env_override;
            command_so_far.env(
                variable_name,
                variable_value.replace(CURRENT_PATH_PLACEHOLDER, &current_path),
            )
        })
}

fn opening_marker(crate_name: &str) -> String {
    format!("<!-- e2e:{crate_name} -->")
}

fn sync_readme_block(
    readme_path: &Path,
    crate_name: &str,
    frequent_output: &str,
    update_requested: bool,
) -> Result<(), Failed> {
    let readme_text = fs::read_to_string(readme_path)?;
    let opening_marker = opening_marker(crate_name);
    let Some(block_start) = readme_text.find(&opening_marker) else {
        return Err(format!("{} has no {opening_marker}", readme_path.display()).into());
    };
    let content_start = block_start + opening_marker.len();
    let Some(content_length) = readme_text[content_start..].find(CLOSING_MARKER) else {
        return Err(format!(
            "{} has no {CLOSING_MARKER} after {opening_marker}",
            readme_path.display()
        )
        .into());
    };
    let recorded_block = &readme_text[content_start..content_start + content_length];
    let expected_block = format!("\n```text\n{}\n```\n", frequent_output.trim_end());
    if recorded_block == expected_block {
        return Ok(());
    }
    if update_requested {
        let synced_text = format!(
            "{}{expected_block}{}",
            &readme_text[..content_start],
            &readme_text[content_start + content_length..]
        );
        fs::write(readme_path, synced_text)?;
        return Ok(());
    }
    Err(format!(
        "{} is out of date; run `cargo xtask update-e2e`\n--- recorded\n{recorded_block}\n--- \
         actual\n{expected_block}",
        readme_path.display()
    )
    .into())
}
