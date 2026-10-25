use std::process::ExitCode;

use cargo_frequent::command_line::{Cli, run};

fn main() -> ExitCode {
    let cli = Cli::parse_cargo_subcommand_arguments();
    match run(&cli) {
        Ok(report_text) => {
            println!("{}", report_text.trim_end());
            ExitCode::SUCCESS
        }
        Err(frequent_error) => {
            eprintln!("Error: {frequent_error}");
            ExitCode::FAILURE
        }
    }
}
