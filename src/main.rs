mod cli;
mod config;
mod console;
mod error;
mod git;
mod init;

use clap::Parser;

use crate::cli::{Cli, Command};
use crate::console::Console;

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let console = Console::from_cli(&cli);

    match cli.command {
        Command::Init => match init::handler(console) {
            Ok(path) => {
                println!("initialized peer in {}", path.display());
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("error: {err}");
                console.debug(format!("{err:?}"));
                ExitCode::FAILURE
            }
        },
        Command::Review { .. } => unimplemented!(),
        Command::Extract { .. } => unimplemented!(),
        Command::Check { .. } => unimplemented!(),
        Command::Render { .. } => unimplemented!(),
    }
}
