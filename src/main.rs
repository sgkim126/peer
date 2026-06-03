mod cli;
mod config;
mod console;
mod error;
mod git;

use clap::Parser;

use crate::cli::{Cli, Command};
use crate::console::Console;

use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let _console = Console::from_cli(&cli);

    match cli.command {
        Command::Init => unimplemented!(),
        Command::Review { .. } => unimplemented!(),
        Command::Extract { .. } => unimplemented!(),
        Command::Check { .. } => unimplemented!(),
        Command::Render { .. } => unimplemented!(),
    }
}
