use std::io;
use std::process::ExitCode;

use capacity_cli::{Arguments, run};
use clap::Parser;

fn main() -> ExitCode {
    let arguments = Arguments::parse();
    let mut stdout = io::stdout().lock();
    let mut stderr = io::stderr().lock();
    ExitCode::from(run(arguments, &mut stdout, &mut stderr))
}
