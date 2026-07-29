use std::process::ExitCode;

use clap::Parser;
use concord::cli::{Cli, exit_code, run};

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = run(cli);
    let code = exit_code(&result);
    if let Err(error) = result {
        eprintln!("error: {error}");
    }
    ExitCode::from(code)
}
