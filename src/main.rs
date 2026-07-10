mod cache;
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
use crate::config::{Config, discover};
use crate::console::Console;
use crate::llm::checks::{CheckCommandError, CheckCommandOutput};
use crate::llm::result::{CheckOutcome, CheckUsage};

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
        Command::Review {
            target,
            provider,
            model,
            skip_checks,
            title,
            body_file,
            comments_file,
            format,
            repo,
        } => {
            let render_options = match render::RenderOptions::from_cli(format, repo) {
                Ok(options) => options,
                Err(error) => {
                    eprintln!("error: {error}");
                    return ExitCode::FAILURE;
                }
            };
            if title.is_none() {
                eprintln!("warning: review title was not provided.");
            }
            if body_file.is_none() {
                eprintln!("warning: review body file was not provided.");
            }
            if comments_file.is_none() {
                eprintln!("warning: review comments file was not provided.");
            }
            let review_context_input = match llm::context::ReviewContextInput::load(
                title,
                body_file.as_deref(),
                comments_file.as_deref(),
            ) {
                Ok(input) => input,
                Err(err) => {
                    eprintln!("error: {err}");
                    console.debug(format_args!("{err:?}"));
                    return ExitCode::FAILURE;
                }
            };

            let cwd = match std::env::current_dir() {
                Ok(cwd) => cwd,
                Err(err) => {
                    eprintln!("cannot determine current directory.");
                    console.debug(format_args!("{err:?}"));
                    return ExitCode::FAILURE;
                }
            };
            let (mut config, project_root) = match discover(&cwd) {
                Ok((config, project_root)) => (config, project_root),
                Err(err) => {
                    eprintln!("{err}");
                    console.debug(format_args!("{err:?}"));
                    return ExitCode::FAILURE;
                }
            };
            apply_llm_defaults(&mut config, provider, model.clone());

            let review_target = match review::resolve_target(&target, &project_root, console).await
            {
                Ok(target) => target,
                Err(err) => {
                    eprintln!("error: {err}");
                    console.debug(format_args!("{err:?}"));
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
                console.debug(format_args!("{err:?}"));
                return ExitCode::FAILURE;
            }

            let plan = review::plan_checks(&review_target).without_checks(&skip_checks);
            console.debug(format_args!("{plan:?}"));

            let (provider_config, model_config) =
                match config.resolve_provider(&config.llm.default_provider, model.as_deref()) {
                    Ok(resolved) => resolved,
                    Err(err) => {
                        eprintln!("{err}");
                        console.debug(format_args!("{err:?}"));
                        return ExitCode::FAILURE;
                    }
                };
            let provider = match llm::provider::create_provider(
                &provider_config.name,
                &provider_config.api_key_env,
                provider_config.base_url.as_deref(),
                console,
            ) {
                Ok(provider) => provider,
                Err(err) => {
                    eprintln!("error: {err}");
                    console.debug(format_args!("{err:?}"));
                    return ExitCode::FAILURE;
                }
            };
            let cache_store = cache::CacheStore::new(project_root.join(".peer/cache"), console);
            let prepared_review_context = match llm::tools::prepare_review_context(
                &provider,
                &provider_config.name,
                &model_config.name,
                review_context_input,
                &cache_store,
            )
            .await
            {
                Ok(context) => context,
                Err(err) => {
                    eprintln!("error: {err}");
                    console.debug(format_args!("{err:?}"));
                    return ExitCode::FAILURE;
                }
            };
            let review_context_usage = CheckUsage::from_raw_usage(
                prepared_review_context.usage,
                model_config.name.clone(),
                model_config.input_per_1m_usd,
                model_config.output_per_1m_usd,
            );
            console.verbose(format_args!(
                "Review context usage: {} input, {} output, ${:.6} ({})",
                review_context_usage.input_tokens,
                review_context_usage.output_tokens,
                review_context_usage.cost_usd,
                review_context_usage.model
            ));
            let review_context = prepared_review_context.context;
            let result = review::run(plan, console, &config, project_root, &review_context).await;
            for error in &result.errors {
                eprintln!("error: {error}");
                console.debug(format_args!("{error:?}"));
            }

            let rendered = render::render_review_result(&result, render_options, console);
            match rendered {
                Ok(rendered) => {
                    println!("{rendered}");
                    review_exit_code(&result)
                }
                Err(err) => {
                    eprintln!("failed to render review output: {err}");
                    console.debug(format_args!("{err:?}"));
                    ExitCode::FAILURE
                }
            }
        }
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
                    println!("{}", serde_json::to_string_pretty(&data).unwrap());
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
            provider,
            model,
            command,
        } => {
            let cwd = match std::env::current_dir() {
                Ok(cwd) => cwd,
                Err(err) => {
                    eprintln!("cannot determine current directory.");
                    console.debug(format_args!("{err:?}"));
                    return ExitCode::FAILURE;
                }
            };
            let result = match discover(&cwd) {
                Ok((mut config, project_root)) => {
                    apply_llm_defaults(&mut config, provider, model);
                    llm::checks::handler(
                        console,
                        command,
                        &config,
                        project_root,
                        &llm::context::ReviewContext::default(),
                    )
                    .await
                }
                Err(error) => Err(CheckCommandError::from(error)),
            };

            if let Err(error) = &result {
                console.debug(format_args!("{error:?}"));
            }
            print_check_result(result, console)
        }
        Command::Render { format, repo } => {
            let render_options = match render::RenderOptions::from_cli(format, repo) {
                Ok(options) => options,
                Err(error) => {
                    eprintln!("error: {error}");
                    return ExitCode::FAILURE;
                }
            };
            let mut input = String::new();
            if let Err(error) = std::io::stdin().read_to_string(&mut input) {
                eprintln!("failed to read render input: {error}");
                return ExitCode::FAILURE;
            }

            match render::render(&input, render_options, console) {
                Ok(output) => {
                    println!("{output}");
                }
                Err(err) => {
                    console.debug(format_args!("{err:?}"));
                    eprintln!("failed to render: {err}");
                    return ExitCode::FAILURE;
                }
            }
            ExitCode::SUCCESS
        }
    }
}

fn apply_llm_defaults(config: &mut Config, provider: Option<String>, model: Option<String>) {
    if let Some(provider) = provider {
        config.llm.default_provider = provider;
    }
    if let Some(model) = model
        && let Some(provider) = config
            .providers
            .iter_mut()
            .find(|provider| provider.name == config.llm.default_provider)
    {
        provider.default_model = model;
    }
}

fn print_check_result(
    result: Result<CheckOutcome, CheckCommandError>,
    console: Console,
) -> ExitCode {
    let exit_code = check_exit_code(&result);
    let output = CheckCommandOutput::from(result);

    match render::render_check_output(&output, render::RenderOptions::Json, console) {
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

fn review_exit_code(result: &review::ReviewResult) -> ExitCode {
    if result.errors.is_empty() && result.outcomes.iter().all(is_complete_outcome) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn check_exit_code(result: &Result<CheckOutcome, CheckCommandError>) -> ExitCode {
    match result {
        Ok(outcome) if is_complete_outcome(outcome) => ExitCode::SUCCESS,
        Ok(_) | Err(_) => ExitCode::FAILURE,
    }
}

fn is_complete_outcome(outcome: &CheckOutcome) -> bool {
    matches!(outcome, CheckOutcome::Success { .. })
}

#[cfg(test)]
mod tests {
    use crate::config::{Config, DEFAULT_CONFIG_TOML};
    use crate::git::CommitHash;
    use crate::llm::result::{
        CheckOutcome, CheckResult, CheckTarget, CheckUsage, CheckUserInfoRequest,
    };
    use crate::review::ReviewResult;
    use std::process::ExitCode;

    use super::{apply_llm_defaults, check_exit_code, review_exit_code};

    #[test]
    fn provider_override_uses_that_providers_default_model() {
        let mut config: Config = toml::from_str(DEFAULT_CONFIG_TOML).unwrap();

        apply_llm_defaults(&mut config, Some("openai".into()), None);

        let (provider, model) = config
            .resolve_provider(&config.llm.default_provider, None)
            .unwrap();
        assert_eq!(provider.name, "openai");
        assert_eq!(model.name, "gpt-5.4-mini");
    }

    #[test]
    fn model_override_replaces_the_selected_providers_default_model() {
        let mut config: Config = toml::from_str(DEFAULT_CONFIG_TOML).unwrap();

        apply_llm_defaults(&mut config, Some("openai".into()), Some("gpt-5.4".into()));

        let (provider, model) = config
            .resolve_provider(&config.llm.default_provider, Some("gpt-5.4"))
            .unwrap();
        assert_eq!(provider.name, "openai");
        assert_eq!(model.name, "gpt-5.4");
    }

    fn success_outcome() -> CheckOutcome {
        CheckOutcome::success(CheckResult {
            check: "size".to_string(),
            target: CheckTarget::Commit(CommitHash::new("abc1234").unwrap()),
            summary: "ok".to_string(),
            findings: Vec::new(),
            confidence: 1.0.try_into().unwrap(),
            iterations: 1,
            is_exhausted: false,
            exhaustion_reason: None,
            usage: usage(),
        })
    }

    fn needs_user_info_outcome() -> CheckOutcome {
        CheckOutcome::NeedsUserInfo {
            request: CheckUserInfoRequest {
                check: "security".to_string(),
                target: CheckTarget::Commit(CommitHash::new("abc1234").unwrap()),
                questions: vec![
                    "Which production auth policy applies here, and why is it needed?".to_string(),
                ],
                iterations: 1,
                usage: usage(),
            },
        }
    }

    fn usage() -> CheckUsage {
        CheckUsage {
            input_tokens: 1,
            output_tokens: 1,
            cost_usd: 0.0,
            model: "test-model".to_string(),
        }
    }

    #[test]
    fn check_exit_succeeds_only_for_complete_outcome() {
        assert_eq!(check_exit_code(&Ok(success_outcome())), ExitCode::SUCCESS);
        assert_eq!(
            check_exit_code(&Ok(needs_user_info_outcome())),
            ExitCode::FAILURE
        );
    }

    #[test]
    fn review_exit_fails_when_any_outcome_needs_user_info() {
        let complete = ReviewResult {
            outcomes: vec![success_outcome()],
            errors: Vec::new(),
        };
        assert_eq!(review_exit_code(&complete), ExitCode::SUCCESS);

        let incomplete = ReviewResult {
            outcomes: vec![success_outcome(), needs_user_info_outcome()],
            errors: Vec::new(),
        };
        assert_eq!(review_exit_code(&incomplete), ExitCode::FAILURE);
    }
}
