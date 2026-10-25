mod showcase_sync;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

use crate::showcase_sync::{SyncMode, sync_showcase_readmes};

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
    #[command(
        about = "Run cargo frequent on every showcase workspace and write its output into the \
                 READMEs"
    )]
    SyncShowcase {
        #[arg(long, help = "Fail when a README is stale instead of rewriting it")]
        check: bool,
    },
}

fn main() -> ExitCode {
    let command_line = XtaskCommandLine::parse();
    let task_result = match command_line.maintenance_task {
        MaintenanceTask::SyncShowcase { check } => sync_showcase_readmes(if check {
            SyncMode::Check
        } else {
            SyncMode::Rewrite
        }),
    };
    match task_result {
        Ok(()) => ExitCode::SUCCESS,
        Err(sync_error) => {
            eprintln!("Error: {sync_error}");
            ExitCode::FAILURE
        }
    }
}
