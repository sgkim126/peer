mod cache;
mod check;
mod cli;
mod config;
mod console;
mod context;
mod error;
mod extract;
mod git;
mod init;
mod llm;
mod pi;
mod render;
mod review;
mod secret;

use std::io::Read;
use std::process::ExitCode;

use clap::Parser;

use crate::cache::CacheStore;
use crate::cli::{Cli, Command};
use crate::config::{Config, discover, discover_peer_root};
use crate::console::Console;
use crate::error::PeerError;
use crate::llm::ProviderKind;
use crate::pi::PiRuntime;

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
        Command::Prune { all } => {
            let cwd = match std::env::current_dir() {
                Ok(cwd) => cwd,
                Err(error) => {
                    eprintln!("cannot determine current directory.");
                    console.debug(format_args!("{error:?}"));
                    return ExitCode::FAILURE;
                }
            };
            let project_root = match discover_peer_root(&cwd) {
                Ok(project_root) => project_root,
                Err(error) => {
                    eprintln!("error: {error}");
                    console.debug(format_args!("{error:?}"));
                    return ExitCode::FAILURE;
                }
            };
            let cache = CacheStore::new(project_root.join(".peer/cache"), console);
            match cache.prune(all) {
                Ok(removed) => {
                    if all {
                        println!("pruned {removed} cache entries");
                    } else {
                        println!("pruned {removed} old cache version directories");
                    }
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("error: {error}");
                    console.debug(format_args!("{error:?}"));
                    ExitCode::FAILURE
                }
            }
        }
        Command::Review {
            target,
            provider,
            model,
            skip_checks,
            only_checks,
            title,
            body_file,
            comments_file,
            no_resume,
            format,
            repo,
        } => {
            let options = match render::RenderOptions::from_cli(format, repo) {
                Ok(options) => options,
                Err(error) => {
                    eprintln!("failed to configure render: {error}");
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
            let review_context = match context::ReviewContext::load(
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
                    eprintln!("cannot determine current directory.");
                    console.debug(format_args!("{error:?}"));
                    return ExitCode::FAILURE;
                }
            };
            let (mut config, project_root) = match discover(&cwd) {
                Ok(discovered) => discovered,
                Err(error) => {
                    eprintln!("{error}");
                    console.debug(format_args!("{error:?}"));
                    return ExitCode::FAILURE;
                }
            };
            if let Err(error) = apply_llm_overrides(&mut config, provider, model) {
                eprintln!("error: {error}");
                console.debug(format_args!("{error:?}"));
                return ExitCode::FAILURE;
            }
            let target = match review::resolve_target(
                &target,
                config.review.max_commits.get(),
                &project_root,
                console,
            )
            .await
            {
                Ok(target) => target,
                Err(error) => {
                    eprintln!("error: {error}");
                    console.debug(format_args!("{error:?}"));
                    return ExitCode::FAILURE;
                }
            };
            if let Err(error) = review::validate_target(
                &target,
                config.review.max_commits.get(),
                &project_root,
                console,
            )
            .await
            {
                eprintln!("error: {error}");
                console.debug(format_args!("{error:?}"));
                return ExitCode::FAILURE;
            }

            let plan = match review::plan_checks(&target)
                .with_only_check(&only_checks)
                .and_then(|plan| plan.excluding_check(&skip_checks))
            {
                Ok(plan) => plan,
                Err(error) => {
                    eprintln!("error: {error}");
                    return ExitCode::FAILURE;
                }
            };
            console.debug(format_args!("{plan:?}"));
            let cache = CacheStore::new(project_root.join(".peer/cache"), console);
            let mut pi = PiRuntime::new(&project_root, cache.clone(), console);
            let compression = match context::compress_review_context(
                &review_context,
                &config,
                &cache,
                &mut pi,
                !no_resume,
                console,
            )
            .await
            {
                Ok(compression) => compression,
                Err(error) => {
                    eprintln!("error: {error}");
                    console.debug(format_args!("{error:?}"));
                    if let Some(usage) = error.usage() {
                        console.verbose(format_args!(
                            "{} context model cost: ${:.6} (input {} tokens, output {} tokens)",
                            usage.model, usage.cost_usd, usage.input_tokens, usage.output_tokens,
                        ));
                    }
                    return ExitCode::FAILURE;
                }
            };
            let result = review::run(
                plan,
                console,
                &config,
                project_root,
                &cache,
                &compression.digest,
                review::ReviewOptions {
                    context_usage: compression.usage,
                    resume: !no_resume,
                },
            )
            .await;
            if let Some(usage) = &result.context_usage {
                console.verbose(format_args!(
                    "{} context model cost: ${:.6} (input {} tokens, output {} tokens)",
                    usage.model, usage.cost_usd, usage.input_tokens, usage.output_tokens,
                ));
            }
            for check in &result.checks {
                console.verbose(format_args!(
                    "{} check for {}: {} model cost: ${:.6} (input {} tokens, output {} tokens)",
                    check.check,
                    check.target,
                    check.usage.model,
                    check.usage.cost_usd,
                    check.usage.input_tokens,
                    check.usage.output_tokens,
                ));
            }
            for (model, usage) in
                review::usage_by_model(&result.checks, result.context_usage.as_ref())
            {
                console.verbose(format_args!(
                    "{model} model total cost: ${:.6} (input {} tokens, output {} tokens)",
                    usage.cost_usd, usage.input_tokens, usage.output_tokens,
                ));
            }
            for error in &result.errors {
                eprintln!("error: {error}");
                console.debug(format_args!("{error:?}"));
            }
            let is_success = result.is_success();

            match render::render(result.into(), options) {
                Ok(output) => {
                    println!("{output}");
                    if is_success {
                        ExitCode::SUCCESS
                    } else {
                        ExitCode::FAILURE
                    }
                }
                Err(error) => {
                    eprintln!("failed to render review output: {error}");
                    console.debug(format_args!("{error:?}"));
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
            provider,
            model,
            title,
            body_file,
            comments_file,
            no_resume,
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
            let review_context = match context::ReviewContext::load(
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
                Ok((mut config, project_root)) => {
                    if let Err(error) = apply_llm_overrides(&mut config, provider, model) {
                        eprintln!("error: {error}");
                        console.debug(format_args!("{error:?}"));
                        return ExitCode::FAILURE;
                    }
                    let review_head =
                        match check::resolve_review_head(&command, &project_root, console).await {
                            Ok(review_head) => review_head,
                            Err(error) => {
                                eprintln!("error: {error}");
                                console.debug(format_args!("{error:?}"));
                                return ExitCode::FAILURE;
                            }
                        };
                    let cache = CacheStore::new(project_root.join(".peer/cache"), console);
                    let mut pi = PiRuntime::new(&project_root, cache.clone(), console);
                    let compression = match context::compress_review_context(
                        &review_context,
                        &config,
                        &cache,
                        &mut pi,
                        !no_resume,
                        console,
                    )
                    .await
                    {
                        Ok(compression) => compression,
                        Err(error) => {
                            eprintln!("error: {error}");
                            console.debug(format_args!("{error:?}"));
                            if let Some(usage) = error.usage() {
                                console.verbose(format_args!(
                                    "{} context model cost: ${:.6} (input {} tokens, output {} tokens)",
                                    usage.model,
                                    usage.cost_usd,
                                    usage.input_tokens,
                                    usage.output_tokens,
                                ));
                            }
                            return ExitCode::FAILURE;
                        }
                    };
                    check::handler(
                        console,
                        command,
                        &config,
                        project_root,
                        &cache,
                        &compression.digest,
                        check::CheckOptions {
                            context_usage: compression.usage,
                            resume: !no_resume,
                            review_head,
                        },
                    )
                    .await
                }
                Err(error) => Err(check::CheckCommandError::Config(error)),
            };
            match result {
                Ok(result) => {
                    let is_success = result.is_success();
                    if let Some(usage) = &result.context_usage {
                        console.verbose(format_args!(
                            "{} context model cost: ${:.6} (input {} tokens, output {} tokens)",
                            usage.model, usage.cost_usd, usage.input_tokens, usage.output_tokens,
                        ));
                    }
                    console.verbose(format_args!(
                        "{} check model cost: ${:.6} (input {} tokens, output {} tokens)",
                        result.usage.model,
                        result.usage.cost_usd,
                        result.usage.input_tokens,
                        result.usage.output_tokens,
                    ));
                    let document = render::RenderDocument::from(result);
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&document)
                            .expect("render document serializes")
                    );
                    if is_success {
                        ExitCode::SUCCESS
                    } else {
                        ExitCode::FAILURE
                    }
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

            let document = match serde_json::from_str::<render::RenderDocument>(&input) {
                Ok(document) => document,
                Err(error) => {
                    eprintln!("failed to parse render document: {error}");
                    console.debug(format_args!("{error:?}"));
                    return ExitCode::FAILURE;
                }
            };
            match render::render(document, options) {
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

fn apply_llm_overrides(
    config: &mut Config,
    provider: Option<ProviderKind>,
    model: Option<String>,
) -> Result<(), PeerError> {
    let (provider_name, model_name) = {
        let (provider, model_config) = config.resolve_provider(provider, model.as_deref())?;
        (provider.name.clone(), model_config.name.clone())
    };

    config.llm.default_provider = provider_name.clone();
    if model.is_some() {
        let provider = config
            .providers
            .iter_mut()
            .find(|provider| provider.name == provider_name)
            .ok_or_else(|| {
                PeerError::invalid_config(format!("provider '{provider_name}' not found in config"))
            })?;
        provider.default_model = model_name;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::DEFAULT_CONFIG_TOML;

    #[test]
    fn provider_override_uses_the_selected_providers_default_model() {
        let mut config: Config = toml::from_str(DEFAULT_CONFIG_TOML).unwrap();

        apply_llm_overrides(&mut config, Some(ProviderKind::OpenAi), None).unwrap();

        let (provider, model) = config.resolve_provider(None, None).unwrap();
        assert_eq!(provider.name, "openai");
        assert_eq!(model.name, "gpt-5.6-luna");
    }

    #[test]
    fn model_override_replaces_the_selected_providers_default_model() {
        let mut config: Config = toml::from_str(DEFAULT_CONFIG_TOML).unwrap();

        apply_llm_overrides(
            &mut config,
            Some(ProviderKind::OpenAi),
            Some("gpt-5.6-terra".into()),
        )
        .unwrap();

        let (provider, model) = config.resolve_provider(None, None).unwrap();
        assert_eq!(provider.name, "openai");
        assert_eq!(model.name, "gpt-5.6-terra");
    }

    #[test]
    fn rejects_an_unconfigured_provider_without_mutating_config() {
        let mut config: Config = toml::from_str(DEFAULT_CONFIG_TOML).unwrap();
        config
            .providers
            .retain(|provider| provider.name != "openai");
        let original_provider = config.llm.default_provider.clone();

        let error = apply_llm_overrides(&mut config, Some(ProviderKind::OpenAi), None).unwrap_err();

        assert_eq!(error.to_string(), "provider 'openai' not found in config");
        assert_eq!(config.llm.default_provider, original_provider);
    }

    #[test]
    fn rejects_an_unconfigured_model_without_mutating_config() {
        let mut config: Config = toml::from_str(DEFAULT_CONFIG_TOML).unwrap();
        let original_model = config
            .providers
            .iter()
            .find(|provider| provider.name == "openai")
            .unwrap()
            .default_model
            .clone();

        let error = apply_llm_overrides(
            &mut config,
            Some(ProviderKind::OpenAi),
            Some("unknown".into()),
        )
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "model 'unknown' not found in provider 'openai'"
        );
        assert_eq!(
            config
                .providers
                .iter()
                .find(|provider| provider.name == "openai")
                .unwrap()
                .default_model,
            original_model
        );
    }
}
