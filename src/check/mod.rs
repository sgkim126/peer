pub mod runner;

use std::fmt;
use std::path::PathBuf;

use crate::cli::CheckCommand;
use crate::config::Config;
use crate::console::Console;
use crate::extract::{ExtractError, Extractor};
use crate::git::CommitHash;
use crate::llm::agent::AgentRequest;
use crate::llm::provider::{ProviderCreationError, ProviderRuntime};
use crate::llm::result::CheckTarget;
use runner::{CheckRunConfig, CheckRunError, Checker};

pub trait CheckDefinition {
    fn name(&self) -> &'static str;
    fn target(&self) -> CheckTarget;
    fn expected_commits(&self) -> &[CommitHash];
    async fn agent_request(
        &self,
        extractor: &Extractor,
        model: &str,
    ) -> Result<AgentRequest, ExtractError>;
}

#[derive(Debug)]
pub enum CheckCommandError {
    Config(crate::error::PeerError),
    Provider(ProviderCreationError),
    Run(CheckRunError),
}

impl fmt::Display for CheckCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => error.fmt(f),
            Self::Provider(error) => error.fmt(f),
            Self::Run(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for CheckCommandError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Config(error) => Some(error),
            Self::Provider(error) => Some(error),
            Self::Run(error) => Some(error),
        }
    }
}

impl From<crate::error::PeerError> for CheckCommandError {
    fn from(error: crate::error::PeerError) -> Self {
        Self::Config(error)
    }
}

impl From<ProviderCreationError> for CheckCommandError {
    fn from(error: ProviderCreationError) -> Self {
        Self::Provider(error)
    }
}

impl From<CheckRunError> for CheckCommandError {
    fn from(error: CheckRunError) -> Self {
        Self::Run(error)
    }
}

impl From<ExtractError> for CheckCommandError {
    fn from(error: ExtractError) -> Self {
        Self::Run(CheckRunError::Preparation(error))
    }
}

pub async fn handler(
    console: Console,
    command: CheckCommand,
    config: &Config,
    project_root: PathBuf,
) -> Result<crate::llm::result::CheckResult, CheckCommandError> {
    let extractor = Extractor::new(project_root, console);

    let check: Check = match command {
        CheckCommand::Size { .. } => Check::Unimplemented,
        CheckCommand::Intent { .. } => Check::Unimplemented,
        CheckCommand::Quality { .. } => Check::Unimplemented,
        CheckCommand::Security { .. } => Check::Unimplemented,
        CheckCommand::Coherence { .. } => Check::Unimplemented,
    };
    let (provider_config, model_config) =
        config.resolve_provider(&config.llm.default_provider, None)?;
    let runtime = ProviderRuntime::try_new(
        &provider_config.name,
        &provider_config.api_key_env,
        provider_config.base_url.as_deref(),
        console,
    )?;
    let result = Checker::new(
        extractor,
        runtime,
        CheckRunConfig {
            model: model_config.name.clone(),
            max_iterations: config.max_iterations_for(check.name()).get(),
            input_per_1m_usd: model_config.input_per_1m_usd,
            output_per_1m_usd: model_config.output_per_1m_usd,
            console,
        },
    )
    .run(&check)
    .await?;
    Ok(result)
}

enum Check {
    Unimplemented,
}

impl CheckDefinition for Check {
    fn name(&self) -> &'static str {
        unimplemented!();
    }

    fn target(&self) -> CheckTarget {
        unimplemented!();
    }

    fn expected_commits(&self) -> &[CommitHash] {
        unimplemented!();
    }

    async fn agent_request(
        &self,
        _extractor: &Extractor,
        _model: &str,
    ) -> Result<AgentRequest, ExtractError> {
        unimplemented!();
    }
}
