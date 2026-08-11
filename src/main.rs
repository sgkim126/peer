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

use std::io::Read;
use std::process::ExitCode;

use clap::Parser;

use crate::cache::CacheStore;
use crate::cli::{Cli, Command};
use crate::config::{Config, discover, discover_peer_root};
use crate::console::Console;
use crate::error::PeerError;
use crate::pi::{ModelRef, PiRuntime};

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
                    runtime: &mut pi,
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
                            runtime: &mut pi,
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
    provider: Option<String>,
    model: Option<String>,
) -> Result<(), PeerError> {
    let provider = provider.unwrap_or_else(|| config.llm.default_provider.clone());
    let model = model.unwrap_or_else(|| config.llm.default_model.clone());
    let model = ModelRef::try_new(provider, model)
        .map_err(|error| PeerError::invalid_config(error.to_string()))?;
    config.llm.default_provider = model.provider().to_string();
    config.llm.default_model = model.model().to_string();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::assert_matches;

    use crate::config::DEFAULT_CONFIG_TOML;

    #[test]
    fn absent_overrides_keep_both_defaults() {
        let mut config: Config = toml::from_str(DEFAULT_CONFIG_TOML).unwrap();

        apply_llm_overrides(&mut config, None, None).unwrap();

        assert_eq!(config.llm.default_provider, "mistral");
        assert_eq!(config.llm.default_model, "mistral-medium-3.5");
    }

    #[test]
    fn provider_and_model_overrides_are_applied_separately() {
        let mut config: Config = toml::from_str(DEFAULT_CONFIG_TOML).unwrap();

        apply_llm_overrides(
            &mut config,
            Some("openai".into()),
            Some("gpt-5.6-terra".into()),
        )
        .unwrap();

        assert_eq!(config.llm.default_provider, "openai");
        assert_eq!(config.llm.default_model, "gpt-5.6-terra");
    }

    #[test]
    fn overrides_accept_names_outside_any_catalog() {
        let mut config: Config = toml::from_str(DEFAULT_CONFIG_TOML).unwrap();

        apply_llm_overrides(
            &mut config,
            Some("custom".into()),
            Some("namespace/new-model".into()),
        )
        .unwrap();

        assert_eq!(config.llm.default_provider, "custom");
        assert_eq!(config.llm.default_model, "namespace/new-model");
    }

    #[test]
    fn overrides_reject_empty_name() {
        let mut config: Config = toml::from_str(DEFAULT_CONFIG_TOML).unwrap();

        assert_matches!(
            apply_llm_overrides(&mut config, Some(String::new()), None),
            Err(PeerError::InvalidConfig { .. })
        );
    }

    #[test]
    fn overrides_reject_padded_name() {
        let mut config: Config = toml::from_str(DEFAULT_CONFIG_TOML).unwrap();

        assert_matches!(
            apply_llm_overrides(&mut config, None, Some(" padded".into())),
            Err(PeerError::InvalidConfig { .. })
        );
    }

    #[test]
    fn individual_overrides_keep_the_other_default() {
        let mut config: Config = toml::from_str(DEFAULT_CONFIG_TOML).unwrap();

        apply_llm_overrides(&mut config, Some("openai".into()), None).unwrap();
        assert_eq!(config.llm.default_provider, "openai");
        assert_eq!(config.llm.default_model, "mistral-medium-3.5");

        apply_llm_overrides(&mut config, None, Some("gpt-5.6-terra".into())).unwrap();
        assert_eq!(config.llm.default_provider, "openai");
        assert_eq!(config.llm.default_model, "gpt-5.6-terra");
    }
}
