mod cli;
mod config;
mod console;
mod error;
mod extract;
mod git;
mod init;
mod llm;
mod render;
mod review;
mod secret;

use clap::Parser;

use crate::cli::{Cli, Command};
use crate::config::discover;
use crate::console::Console;
use crate::llm::checks::{CheckCommandError, CheckCommandOutput};
use crate::llm::result::CheckResult;

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
                console.debug(format!("{err:?}"));
                ExitCode::FAILURE
            }
        },
        Command::Review { target, format } => {
            let cwd = match std::env::current_dir() {
                Ok(cwd) => cwd,
                Err(err) => {
                    eprintln!("cannot determine current directory.");
                    console.debug(format!("{err:?}"));
                    return ExitCode::FAILURE;
                }
            };
            let (config, project_root) = match discover(&cwd) {
                Ok((config, project_root)) => (config, project_root),
                Err(err) => {
                    eprintln!("{err}");
                    console.debug(format!("{err:?}"));
                    return ExitCode::FAILURE;
                }
            };

            let review_target = match review::resolve_target(&target, &project_root, console).await
            {
                Ok(target) => target,
                Err(err) => {
                    eprintln!("error: {err}");
                    console.debug(format!("{err:?}"));
                    return ExitCode::FAILURE;
                }
            };
            if let Err(err) = review::validate_target(
                &review_target,
                config.review.max_commits,
                &project_root,
                console,
            )
            .await
            {
                eprintln!("error: {err}");
                console.debug(format!("{err:?}"));
                return ExitCode::FAILURE;
            }

            let plan = review::plan_checks(&review_target);
            console.debug(format!("{plan:?}"));

            let result = review::run(plan, console, &config, project_root).await;
            for error in &result.errors {
                eprintln!("error: {error}");
                console.debug(format!("{error:?}"));
            }

            let rendered = render::render_review_result(&result, format, console);
            match rendered {
                Ok(rendered) => {
                    println!("{rendered}");
                    if result.errors.is_empty() {
                        ExitCode::SUCCESS
                    } else {
                        ExitCode::FAILURE
                    }
                }
                Err(err) => {
                    eprintln!("failed to render review output: {err}");
                    console.debug(format!("{err:?}"));
                    ExitCode::FAILURE
                }
            }
        }
        Command::Extract { command } => {
            let cwd = match std::env::current_dir() {
                Ok(cwd) => cwd,
                Err(err) => {
                    eprintln!("cannot determine current directory.");
                    console.debug(format!("{err:?}"));
                    return ExitCode::FAILURE;
                }
            };
            let (config, project_root) = match discover(&cwd) {
                Ok((config, project_root)) => (config, project_root),
                Err(err) => {
                    eprintln!("{err}");
                    console.debug(format!("{err:?}"));
                    return ExitCode::FAILURE;
                }
            };

            match extract::handler(console, &command, config, project_root).await {
                Ok(data) => {
                    println!("{}", serde_json::to_string_pretty(&data).unwrap());
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    eprintln!("error: {err}");
                    console.debug(format!("{err:?}"));
                    ExitCode::FAILURE
                }
            }
        }
        Command::Check { command } => {
            let cwd = match std::env::current_dir() {
                Ok(cwd) => cwd,
                Err(err) => {
                    eprintln!("cannot determine current directory.");
                    console.debug(format!("{err:?}"));
                    return ExitCode::FAILURE;
                }
            };
            let result = match discover(&cwd) {
                Ok((config, project_root)) => {
                    llm::checks::handler(console, command, &config, project_root).await
                }
                Err(error) => Err(CheckCommandError::from(error)),
            };

            if let Err(error) = &result {
                console.debug(format!("{error:?}"));
            }
            print_check_result(result, console)
        }
        Command::Render { format } => {
            let mut input = String::new();
            if let Err(error) = std::io::stdin().read_to_string(&mut input) {
                eprintln!("failed to read render input: {error}");
                return ExitCode::FAILURE;
            }

            match render::render(&input, format, console) {
                Ok(output) => {
                    println!("{output}");
                }
                Err(err) => {
                    console.debug(format!("{err:?}"));
                    eprintln!("failed to render: {err}");
                    return ExitCode::FAILURE;
                }
            }
            ExitCode::SUCCESS
        }
    }
}

fn print_check_result(
    result: Result<CheckResult, CheckCommandError>,
    console: Console,
) -> ExitCode {
    let exit_code = if result.is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    };
    let output = CheckCommandOutput::from(result);

    match render::render_check_output(&output, cli::OutputFormat::Json, console) {
        Ok(json) => {
            println!("{json}");
            exit_code
        }
        Err(error) => {
            eprintln!("failed to serialize check output: {error}");
            ExitCode::FAILURE
        }
    }
}
