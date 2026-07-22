mod check;
mod cli;
mod config;
mod console;
mod error;
mod extract;
mod git;
mod init;
mod llm;
mod secret;

use clap::Parser;

use crate::cli::{Cli, Command};
use crate::config::discover;
use crate::console::Console;

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let console = Console::from_cli(&cli);

    match cli.command {
        Command::Init => match init::handler(console).await {
            Ok(path) => {
                println!("initialized peer in {}", path.display());
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("error: {err}");
                console.debug(format_args!("{err:?}"));
                ExitCode::FAILURE
            }
        },
        Command::Review { .. } => unimplemented!(),
        Command::Extract { command } => {
            let cwd = match std::env::current_dir() {
                Ok(cwd) => cwd,
                Err(err) => {
                    eprintln!("cannot determine current directory.");
                    console.debug(format_args!("{err:?}"));
                    return ExitCode::FAILURE;
                }
            };
            let (config, project_root) = match discover(&cwd) {
                Ok((config, project_root)) => (config, project_root),
                Err(err) => {
                    eprintln!("{err}");
                    console.debug(format_args!("{err:?}"));
                    return ExitCode::FAILURE;
                }
            };

            match extract::handler(console, &command, config, project_root).await {
                Ok(data) => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&data)
                            .expect("serialisation should never fail")
                    );
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    eprintln!("error: {err}");
                    console.debug(format_args!("{err:?}"));
                    ExitCode::FAILURE
                }
            }
        }
        Command::Check { command } => {
            let cwd = match std::env::current_dir() {
                Ok(cwd) => cwd,
                Err(error) => {
                    eprintln!("cannot determine current directory: {error}");
                    return ExitCode::FAILURE;
                }
            };
            let result = match discover(&cwd) {
                Ok((config, project_root)) => {
                    check::handler(console, command, &config, project_root).await
                }
                Err(error) => Err(check::CheckCommandError::Config(error)),
            };
            match result {
                Ok(result) => {
                    console.verbose(format_args!(
                        "{} model cost: ${:.6} (input {} tokens, output {} tokens)",
                        result.usage.model,
                        result.usage.cost_usd,
                        result.usage.input_tokens,
                        result.usage.output_tokens,
                    ));
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&result).expect("check result serializes")
                    );
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("error: {error}");
                    console.debug(format_args!("{error:?}"));
                    ExitCode::FAILURE
                }
            }
        }
        Command::Render { .. } => unimplemented!(),
    }
}
