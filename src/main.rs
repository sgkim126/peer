mod cache;
mod cli;
mod config;
mod context;
mod error;
mod extract;
mod feedback;
mod git;
mod github;
mod gitlab;
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
use crate::cli::{Cli, Command, OutputFormat};
use crate::config::{Config, discover, discover_peer_root};
use crate::error::PeerError;
use crate::pi::{ModelRef, PiRuntime};

#[tokio::main]
async fn main() -> ExitCode {
    init_logging();
    let cli = Cli::parse();

    match cli.command {
        Command::Init {
            provider,
            model,
            repo,
            github: _,
            gitlab,
        } => match init::handler(provider, model, repo, gitlab).await {
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
            github,
            gitlab,
            repo,
            no_resume,
        } => {
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
            if let Some(repo) = repo {
                if gitlab.is_some() {
                    config.gitlab.repo = Some(repo);
                } else {
                    config.github.repo = Some(repo);
                }
            }
            let (remote_commits, review_context, source) = if let Some(number) = github {
                let result = async {
                    let repository = config
                        .github
                        .repo
                        .as_deref()
                        .filter(|value| !value.trim().is_empty())
                        .ok_or(github::GitHubError::MissingRepository)?;
                    let repository = github::Repository::parse(repository)?;
                    github::GitHubClient::from_env()?
                        .review_input(&repository, number)
                        .await
                }
                .await;
                match result {
                    Ok(input) => (Some(input.commits), input.context, None),
                    Err(error) => {
                        eprintln!("error: {error}");
                        debug!("{error:?}");
                        return ExitCode::FAILURE;
                    }
                }
            } else if let Some(number) = gitlab {
                let result = async {
                    let repository = config
                        .gitlab
                        .repo
                        .as_deref()
                        .filter(|value| !value.trim().is_empty())
                        .ok_or(gitlab::GitLabError::MissingRepository)?;
                    let repository = gitlab::Repository::parse(repository)?;
                    gitlab::GitLabClient::from_env()?
                        .review_input(&repository, number)
                        .await
                }
                .await;
                match result {
                    Ok(input) => (Some(input.commits), input.context, Some(input.source)),
                    Err(error) => {
                        eprintln!("error: {error}");
                        debug!("{error:?}");
                        return ExitCode::FAILURE;
                    }
                }
            } else {
                match context::ReviewContext::load(
                    title,
                    body_file.as_deref(),
                    comments_file.as_deref(),
                ) {
                    Ok(context) => (None, context, None),
                    Err(error) => {
                        eprintln!("error: {error}");
                        debug!("{error:?}");
                        return ExitCode::FAILURE;
                    }
                }
            };
            if let Err(error) = apply_llm_overrides(&mut config, provider, model) {
                eprintln!("error: {error}");
                debug!("{error:?}");
                return ExitCode::FAILURE;
            }
            let target = if let Some(source) = &source {
                review::resolve_merge_request_target(
                    remote_commits.expect("GitLab input contains commits"),
                    &source.diff_refs,
                    config.review.max_commits.get(),
                    &project_root,
                )
                .await
            } else if let Some(commits) = remote_commits {
                review::resolve_pull_request_target(
                    commits,
                    config.review.max_commits.get(),
                    &project_root,
                )
                .await
            } else {
                review::resolve_target(
                    target
                        .as_deref()
                        .expect("clap requires target without a remote review"),
                    config.review.max_commits.get(),
                    &project_root,
                )
                .await
            };
            let target = match target {
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
                for usage in stage.usage().iter() {
                    info!(
                        "{} stage for {}: {}/{} model cost: ${:.6} (input {} tokens, output {} tokens)",
                        stage.stage().as_str(),
                        stage.target(),
                        usage.provider,
                        usage.model,
                        usage.cost_usd,
                        usage.input_tokens,
                        usage.output_tokens,
                    );
                }
            }
            for error in &result.errors {
                if let Some(usage) = &error.usage {
                    for usage in usage.iter() {
                        info!(
                            "{} stage for {}: {}/{} model cost: ${:.6} (input {} tokens, output {} tokens)",
                            error.stage.as_str(),
                            error.target,
                            usage.provider,
                            usage.model,
                            usage.cost_usd,
                            usage.input_tokens,
                            usage.output_tokens,
                        );
                    }
                }
                eprintln!("error: {error}");
                debug!("{error:?}");
            }
            let is_success = result.is_success();

            match render::render_pipeline_json(result, source) {
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
        Command::Render {
            format,
            repo,
            github,
            gitlab,
        } => {
            let repo = if matches!(format, OutputFormat::Github | OutputFormat::Gitlab)
                && repo.is_none()
            {
                let cwd = match std::env::current_dir() {
                    Ok(cwd) => cwd,
                    Err(error) => {
                        eprintln!("cannot determine current directory.");
                        debug!("{error:?}");
                        return ExitCode::FAILURE;
                    }
                };
                match discover(&cwd) {
                    Ok((config, _)) => match format {
                        OutputFormat::Github => config.github.repo,
                        OutputFormat::Gitlab => config.gitlab.repo,
                        _ => unreachable!("hosted output"),
                    }
                    .filter(|value| !value.trim().is_empty()),
                    Err(error) => {
                        eprintln!("failed to configure render: {error}");
                        debug!("{error:?}");
                        return ExitCode::FAILURE;
                    }
                }
            } else {
                repo
            };
            let options = match render::RenderOptions::from_cli(format, repo.clone()) {
                Ok(options) => options,
                Err(error) => {
                    eprintln!("failed to configure render: {error}");
                    return ExitCode::FAILURE;
                }
            };
            if github.is_some() && format != OutputFormat::Github {
                eprintln!(
                    "failed to configure render: --github can only be used with --format github"
                );
                return ExitCode::FAILURE;
            }
            if gitlab.is_some() && format != OutputFormat::Gitlab {
                eprintln!(
                    "failed to configure render: --gitlab can only be used with --format gitlab"
                );
                return ExitCode::FAILURE;
            }
            let mut input = String::new();
            if let Err(error) = std::io::stdin().read_to_string(&mut input) {
                eprintln!("failed to read render input: {error}");
                debug!("{error:?}");
                return ExitCode::FAILURE;
            }

            let input = match serde_json::from_str::<render::RenderDocument>(&input) {
                Ok(input) => input,
                Err(error) => {
                    eprintln!("failed to parse render input: {error}");
                    debug!("{error:?}");
                    return ExitCode::FAILURE;
                }
            };
            if let Some(number) = github {
                let result = async {
                    let repository = github::Repository::parse(
                        repo.as_deref()
                            .expect("GitHub rendering requires a repository"),
                    )?;
                    github::GitHubClient::from_env()?
                        .publish(&repository, number, &input)
                        .await
                }
                .await;
                return match result {
                    Ok(report) => {
                        println!("{report}");
                        ExitCode::SUCCESS
                    }
                    Err(error) => {
                        eprintln!("failed to publish review: {error}");
                        debug!("{error:?}");
                        ExitCode::FAILURE
                    }
                };
            }
            if let Some(number) = gitlab {
                let result: Result<_, gitlab::PublishError> = async {
                    let repository = gitlab::Repository::parse(
                        repo.as_deref()
                            .expect("GitLab rendering requires a repository"),
                    )?;
                    let client = gitlab::GitLabClient::from_env()?;
                    client.publish(&repository, number, &input).await
                }
                .await;
                return match result {
                    Ok(report) => {
                        println!("{report}");
                        ExitCode::SUCCESS
                    }
                    Err(error) => {
                        eprintln!("failed to publish review: {error}");
                        debug!("{error:?}");
                        ExitCode::FAILURE
                    }
                };
            }
            match render::render(input, options) {
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
