use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use fake_app_server::{Scenario, run_scenario};
use tokio::io::{self, BufReader};

#[derive(Debug, Parser)]
#[command(name = "fake-app-server")]
#[command(about = "Replay a deterministic Codex app-server JSONL scenario")]
struct Arguments {
    #[arg(long)]
    scenario: PathBuf,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let arguments = Arguments::parse();
    let scenario = match Scenario::from_directory(&arguments.scenario) {
        Ok(scenario) => scenario,
        Err(error) => {
            eprintln!("fake-app-server: {error}");
            return ExitCode::from(2);
        }
    };

    let stdin = io::stdin();
    let mut stdout = io::stdout();
    match run_scenario(&scenario, BufReader::new(stdin), &mut stdout).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fake-app-server: {error}");
            ExitCode::from(3)
        }
    }
}
