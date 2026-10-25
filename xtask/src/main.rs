use std::{path::Path, process::ExitCode};

use clap::{Parser, Subcommand};
use xshell::{Shell, cmd};

#[derive(Debug, Parser)]
#[command(
    name = "xtask",
    about = "Repository maintenance tasks for cargo-frequent"
)]
struct XtaskCommandLine {
    #[command(subcommand)]
    maintenance_task: MaintenanceTask,
}

#[derive(Debug, Subcommand)]
enum MaintenanceTask {
    #[command(about = "Run every e2e test crate and rewrite the recorded output in its README")]
    UpdateE2e,
}

fn main() -> ExitCode {
    let command_line = XtaskCommandLine::parse();
    let task_result = match command_line.maintenance_task {
        MaintenanceTask::UpdateE2e => update_e2e_outputs(),
    };
    match task_result {
        Ok(()) => ExitCode::SUCCESS,
        Err(shell_error) => {
            eprintln!("Error: {shell_error}");
            ExitCode::FAILURE
        }
    }
}

fn update_e2e_outputs() -> xshell::Result<()> {
    let maintenance_shell = Shell::new()?;
    let xtask_directory = Path::new(env!("CARGO_MANIFEST_DIR"));
    maintenance_shell.change_dir(xtask_directory.parent().unwrap_or(xtask_directory));
    cmd!(
        maintenance_shell,
        "cargo test --package cargo-frequent --test e2e_crates"
    )
    .env("UPDATE_E2E", "1")
    .run()
}
