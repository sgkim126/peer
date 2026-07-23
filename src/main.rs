mod check;
mod cli;
mod config;
mod console;
mod error;
mod extract;
mod git;
mod init;
mod llm;
mod render;
mod secret;

use clap::Parser;

use crate::cli::{Cli, Command};
use crate::config::discover;
use crate::console::Console;

use std::io::Read;
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
        Command::Check {
            title,
            body_file,
            comments_file,
            command,
        } => {
            if title.is_none() {
                eprintln!("warning: review title was not provided.");
            }
            if body_file.is_none() {
                eprintln!("warning: review body file was not provided.");
            }
            if comments_file.is_none() {
                eprintln!("warning: review comments file was not provided.");
            }
            let review_context = match llm::context::ReviewContext::load(
                title,
                body_file.as_deref(),
                comments_file.as_deref(),
            ) {
                Ok(context) => context,
                Err(error) => {
                    eprintln!("error: {error}");
                    console.debug(format_args!("{error:?}"));
                    return ExitCode::FAILURE;
                }
            };
            let cwd = match std::env::current_dir() {
                Ok(cwd) => cwd,
                Err(error) => {
                    eprintln!("cannot determine current directory: {error}");
                    return ExitCode::FAILURE;
                }
            };
            let result = match discover(&cwd) {
                Ok((config, project_root)) => {
                    check::handler(console, command, &config, project_root, &review_context).await
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
        Command::Render { format, repo } => {
            let options = match render::RenderOptions::from_cli(format, repo) {
                Ok(options) => options,
                Err(error) => {
                    eprintln!("failed to configure render: {error}");
                    return ExitCode::FAILURE;
                }
            };
            let mut input = String::new();
            if let Err(error) = std::io::stdin().read_to_string(&mut input) {
                eprintln!("failed to read render input: {error}");
                console.debug(format_args!("{error:?}"));
                return ExitCode::FAILURE;
            }

            match render::render(&input, options) {
                Ok(output) => {
                    println!("{output}");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("failed to render: {error}");
                    console.debug(format_args!("{error:?}"));
                    ExitCode::FAILURE
                }
            }
        }
    }
}
