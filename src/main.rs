use std::process::ExitCode;

use cargo_frequent::command_line::{Cli, run};

fn main() -> ExitCode {
    let cli = Cli::parse_cargo_subcommand_arguments();
    cli.output_color_choice().write_global();
    match run(&cli) {
        Ok(report_text) => {
            anstream::println!("{}", report_text.trim_end());
            ExitCode::SUCCESS
        }
        Err(frequent_error) => {
            anstream::eprintln!("Error: {frequent_error}");
            ExitCode::FAILURE
        }
    }
}
