use std::{
    env,
    path::PathBuf,
    time::{Duration, SystemTime},
};

use clap::Parser;

use crate::{
    frequent_error::FrequentError,
    target_reading::{
        fingerprint_build_log::build_with_fingerprint_log,
        fingerprint_units::scan_fingerprint_units, workspace_metadata::read_workspace_metadata,
    },
    trigger_detection::{
        live_dirty_reasons::detect_live_dirty_reasons,
        recorded_env_drift::detect_recorded_env_drift, unit_hash_churn::detect_unit_hash_churn,
    },
    trigger_graph::{graph_rendering::render_trigger_paths, rebuilt_members::trace_trigger_paths},
};

const CHURN_WINDOW: Duration = Duration::from_hours(24);
const DEFAULT_BUILD_COMMAND: &str = "check";

#[derive(Parser, Debug)]
#[command(
    name = "cargo-frequent",
    bin_name = "cargo frequent",
    version,
    about = "Draw what triggers your crates to recompile, read from target/ without building"
)]
pub struct Cli {
    #[arg(
        long,
        help = "Run `cargo check` first (or the cargo command after --) and include what it \
                recompiled"
    )]
    pub build: bool,

    #[arg(long, help = "Print the graph as JSON")]
    pub json: bool,

    #[arg(long, default_value = ".", help = "Directory of the cargo project")]
    pub path: PathBuf,

    #[arg(
        last = true,
        requires = "build",
        help = "Cargo command and arguments for --build, e.g. -- test -p foo"
    )]
    pub build_command: Vec<String>,
}

impl Cli {
    #[must_use]
    pub fn parse_cargo_subcommand_arguments() -> Self {
        let mut command_arguments: Vec<String> = env::args().collect();
        if command_arguments
            .get(1)
            .is_some_and(|first_argument| first_argument == "frequent")
        {
            command_arguments.remove(1);
        }
        Self::parse_from(command_arguments)
    }
}

pub fn run(cli: &Cli) -> Result<String, FrequentError> {
    let workspace_metadata = read_workspace_metadata(&cli.path)?;
    let live_triggers = if cli.build {
        let build_command = if cli.build_command.is_empty() {
            vec![DEFAULT_BUILD_COMMAND.to_string()]
        } else {
            cli.build_command.clone()
        };
        detect_live_dirty_reasons(&build_with_fingerprint_log(&cli.path, &build_command)?)
    } else {
        Vec::new()
    };
    let fingerprint_units = scan_fingerprint_units(&workspace_metadata.target_directory)?;
    let recent_cutoff = SystemTime::now()
        .checked_sub(CHURN_WINDOW)
        .unwrap_or(SystemTime::UNIX_EPOCH);

    let rebuild_triggers = live_triggers
        .into_iter()
        .chain(detect_recorded_env_drift(
            &fingerprint_units,
            |variable_name| env::var(variable_name).ok(),
        ))
        .chain(detect_unit_hash_churn(
            &fingerprint_units,
            recent_cutoff,
            workspace_metadata.default_members_select_subset,
        ))
        .collect();
    let trigger_paths = trace_trigger_paths(
        rebuild_triggers,
        &fingerprint_units,
        &workspace_metadata.member_names,
    );

    if cli.json {
        return serde_json::to_string_pretty(&trigger_paths)
            .map_err(|json_error| FrequentError::ReportSerialization { json_error });
    }
    Ok(render_trigger_paths(&trigger_paths))
}
