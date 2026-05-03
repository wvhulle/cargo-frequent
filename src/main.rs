use std::process::ExitCode;

use cargo_frequent::Cli;

fn main() -> ExitCode {
    let cli = Cli::parse_args();
    cli.init_logging();

    match cli.analyze() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {e}");
            ExitCode::FAILURE
        }
    }
}
