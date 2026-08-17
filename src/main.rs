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
mod stage;

use std::io::Read;
use std::process::ExitCode;

use clap::Parser;
use log::{debug, info};

use crate::cache::CacheStore;
use crate::cli::{Cli, Command};
use crate::config::{Config, discover, discover_peer_root};
use crate::error::PeerError;
use crate::pi::{ModelRef, PiRuntime};

#[tokio::main]
async fn main() -> ExitCode {
    init_logging();
    let cli = Cli::parse();

    match cli.command {
        Command::Init => match init::handler().await {
            Ok(path) => {
                println!("initialized peer in {}", path.display());
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("error: {err}");
                debug!("{err:?}");
                ExitCode::FAILURE
            }
        },
        Command::Prune { all } => {
            let cwd = match std::env::current_dir() {
                Ok(cwd) => cwd,
                Err(error) => {
                    eprintln!("cannot determine current directory.");
                    debug!("{error:?}");
                    return ExitCode::FAILURE;
                }
            };
            let project_root = match discover_peer_root(&cwd) {
                Ok(project_root) => project_root,
                Err(error) => {
                    eprintln!("error: {error}");
                    debug!("{error:?}");
                    return ExitCode::FAILURE;
                }
            };
            let cache = CacheStore::new(project_root.join(".peer/cache"));
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
                    debug!("{error:?}");
                    ExitCode::FAILURE
                }
            }
        }
        Command::Review {
            target,
            provider,
            model,
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
            let review_context = match context::ReviewContext::load(
                title,
                body_file.as_deref(),
                comments_file.as_deref(),
            ) {
                Ok(context) => context,
                Err(error) => {
                    eprintln!("error: {error}");
                    debug!("{error:?}");
                    return ExitCode::FAILURE;
                }
            };
            let cwd = match std::env::current_dir() {
                Ok(cwd) => cwd,
                Err(error) => {
                    eprintln!("cannot determine current directory.");
                    debug!("{error:?}");
                    return ExitCode::FAILURE;
                }
            };
            let (mut config, project_root) = match discover(&cwd) {
                Ok(discovered) => discovered,
                Err(error) => {
                    eprintln!("{error}");
                    debug!("{error:?}");
                    return ExitCode::FAILURE;
                }
            };
            if let Err(error) = apply_llm_overrides(&mut config, provider, model) {
                eprintln!("error: {error}");
                debug!("{error:?}");
                return ExitCode::FAILURE;
            }
            let target = match review::resolve_target(
                &target,
                config.review.max_commits.get(),
                &project_root,
            )
            .await
            {
                Ok(target) => target,
                Err(error) => {
                    eprintln!("error: {error}");
                    debug!("{error:?}");
                    return ExitCode::FAILURE;
                }
            };
            if let Err(error) =
                review::validate_target(&target, config.review.max_commits.get(), &project_root)
                    .await
            {
                eprintln!("error: {error}");
                debug!("{error:?}");
                return ExitCode::FAILURE;
            }

            let cache = CacheStore::new(project_root.join(".peer/cache"));
            let mut pi = PiRuntime::new(&project_root, cache.clone());
            let result = match review::run_pipeline(
                &target,
                review_context,
                &config,
                project_root,
                &cache,
                &mut pi,
                !no_resume,
            )
            .await
            {
                Ok(result) => result,
                Err(error) => {
                    eprintln!("error: {error}");
                    debug!("{error:?}");
                    return ExitCode::FAILURE;
                }
            };
            for stage in &result.stages {
                info!(
                    "{} stage for {}: {} model cost: ${:.6} (input {} tokens, output {} tokens)",
                    stage.stage().as_str(),
                    stage.target(),
                    stage.usage().model,
                    stage.usage().cost_usd,
                    stage.usage().input_tokens,
                    stage.usage().output_tokens,
                );
            }
            for error in &result.errors {
                if let Some(usage) = &error.usage {
                    info!(
                        "{} stage for {}: {} model cost: ${:.6} (input {} tokens, output {} tokens)",
                        error.stage.as_str(),
                        error.target,
                        usage.model,
                        usage.cost_usd,
                        usage.input_tokens,
                        usage.output_tokens,
                    );
                }
                eprintln!("error: {error}");
                debug!("{error:?}");
            }
            let is_success = result.is_success();

            match render::render_pipeline(result, options) {
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
                    debug!("{error:?}");
                    ExitCode::FAILURE
                }
            }
        }
        Command::Extract { command } => {
            let cwd = match std::env::current_dir() {
                Ok(cwd) => cwd,
                Err(err) => {
                    eprintln!("cannot determine current directory.");
                    debug!("{err:?}");
                    return ExitCode::FAILURE;
                }
            };
            let (config, project_root) = match discover(&cwd) {
                Ok((config, project_root)) => (config, project_root),
                Err(err) => {
                    eprintln!("{err}");
                    debug!("{err:?}");
                    return ExitCode::FAILURE;
                }
            };

            match extract::handler(&command, config, project_root).await {
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
                    debug!("{err:?}");
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
                debug!("{error:?}");
                return ExitCode::FAILURE;
            }

            let document = match serde_json::from_str::<render::RenderDocument>(&input) {
                Ok(document) => document,
                Err(error) => {
                    eprintln!("failed to parse render document: {error}");
                    debug!("{error:?}");
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
                    debug!("{error:?}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}

fn init_logging() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
        .target(env_logger::Target::Stderr)
        .init();
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
