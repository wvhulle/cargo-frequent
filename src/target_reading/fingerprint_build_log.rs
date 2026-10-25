use std::{
    io::{BufRead, BufReader},
    path::Path,
    process::{Command, Stdio},
};

use crate::{
    frequent_error::FrequentError,
    target_reading::{
        cargo_run_variables::inherited_cargo_run_variables,
        dirty_reason_parser::{DirtyUnitEntry, parse_fingerprint_log_line},
    },
};

const FINGERPRINT_LOG_FILTER: &str =
    "cargo::core::compiler::fingerprint=info,cargo::compiler::fingerprint=info";

pub fn build_with_fingerprint_log(
    project_path: &Path,
    cargo_arguments: &[String],
) -> Result<Vec<DirtyUnitEntry>, FrequentError> {
    let mut cargo_command = Command::new("cargo");
    inherited_cargo_run_variables()
        .into_iter()
        .for_each(|inherited_variable| {
            cargo_command.env_remove(inherited_variable);
        });
    let mut cargo_process = cargo_command
        .args(cargo_arguments)
        .current_dir(project_path)
        .env("CARGO_LOG", FINGERPRINT_LOG_FILTER)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|io_error| FrequentError::CargoSpawnFailed { io_error })?;
    let Some(cargo_stderr) = cargo_process.stderr.take() else {
        return Ok(Vec::new());
    };
    let dirty_unit_entries = BufReader::new(cargo_stderr)
        .lines()
        .map_while(Result::ok)
        .filter_map(|log_line| parse_fingerprint_log_line(&log_line))
        .collect();
    cargo_process
        .wait()
        .map_err(|io_error| FrequentError::BuildLogUnreadable { io_error })?;
    Ok(dirty_unit_entries)
}
