mod coherence;
mod intent;
mod quality;
mod runner;
mod security;
mod size;

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::cache::{CacheKey, CacheStore};
use crate::cli::CheckCommand;
use crate::config::Config;
use crate::console::Console;
use crate::context::ReviewContextDigest;
use crate::extract::{ExtractError, Extractor};
use crate::git::CommitHash;
use crate::llm::{
    AgentRequest, CheckResult, CheckTarget, Finding, LlmUsage, ProviderCreationError,
    ProviderRuntime,
};

use self::coherence::CoherenceCheck;
use self::intent::IntentCheck;
use self::quality::QualityCheck;
use self::runner::{CheckRunConfig, CheckRunError, Checker};
use self::security::SecurityCheck;
use self::size::SizeCheck;

trait CheckDefinition {
    fn name(&self) -> &'static str;
    fn target(&self) -> CheckTarget;
    fn expected_commits(&self) -> &[CommitHash];
    async fn agent_request(
        &self,
        extractor: &Extractor,
        model: &str,
        review_context: &ReviewContextDigest,
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
    cache_store: &CacheStore,
    review_context: &ReviewContextDigest,
    context_usage: Option<LlmUsage>,
) -> Result<CheckResult, CheckCommandError> {
    let extractor = Extractor::new(project_root, console);

    let check: Check = match command {
        CheckCommand::Size { revision } => {
            Check::Size(SizeCheck::try_new(&revision, &extractor).await?)
        }
        CheckCommand::Intent { revision } => {
            Check::Intent(IntentCheck::try_new(&revision, &extractor).await?)
        }
        CheckCommand::Quality { revision } => {
            Check::Quality(QualityCheck::try_new(&revision, &extractor).await?)
        }
        CheckCommand::Security { revision } => {
            Check::Security(SecurityCheck::try_new(&revision, &extractor).await?)
        }
        CheckCommand::Coherence { range } => {
            Check::Coherence(CoherenceCheck::try_new(&range, &extractor).await?)
        }
    };
    let (provider_config, model_config) = config.resolve_provider(None, None)?;
    let max_iterations = config.max_iterations_for(check.name()).get();
    let cache_key = check_cache_key(
        &check,
        &provider_config.name,
        &model_config.name,
        review_context,
        console,
    );
    if let Some(key) = &cache_key
        && let Some(result) = load_check_cache(
            cache_store,
            key,
            &check,
            &model_config.name,
            context_usage.clone(),
            console,
        )
    {
        return Ok(result);
    }
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
            max_iterations,
            input_per_1m_usd: model_config.input_per_1m_usd,
            output_per_1m_usd: model_config.output_per_1m_usd,
            context_usage,
            console,
        },
    )
    .run(&check, review_context)
    .await?;
    if let Some(key) = &cache_key {
        store_check_cache(cache_store, key, &result, console);
    }
    Ok(result)
}

enum Check {
    Size(SizeCheck),
    Intent(IntentCheck),
    Quality(QualityCheck),
    Security(SecurityCheck),
    Coherence(CoherenceCheck),
}

impl CheckDefinition for Check {
    fn name(&self) -> &'static str {
        match self {
            Self::Size(check) => check.name(),
            Self::Intent(check) => check.name(),
            Self::Quality(check) => check.name(),
            Self::Security(check) => check.name(),
            Self::Coherence(check) => check.name(),
        }
    }

    fn target(&self) -> CheckTarget {
        match self {
            Self::Size(check) => check.target(),
            Self::Intent(check) => check.target(),
            Self::Quality(check) => check.target(),
            Self::Security(check) => check.target(),
            Self::Coherence(check) => check.target(),
        }
    }

    fn expected_commits(&self) -> &[CommitHash] {
        match self {
            Self::Size(check) => check.expected_commits(),
            Self::Intent(check) => check.expected_commits(),
            Self::Quality(check) => check.expected_commits(),
            Self::Security(check) => check.expected_commits(),
            Self::Coherence(check) => check.expected_commits(),
        }
    }

    async fn agent_request(
        &self,
        extractor: &Extractor,
        model: &str,
        review_context: &ReviewContextDigest,
    ) -> Result<AgentRequest, ExtractError> {
        match self {
            Self::Size(check) => check.agent_request(extractor, model, review_context).await,
            Self::Intent(check) => check.agent_request(extractor, model, review_context).await,
            Self::Quality(check) => check.agent_request(extractor, model, review_context).await,
            Self::Security(check) => check.agent_request(extractor, model, review_context).await,
            Self::Coherence(check) => check.agent_request(extractor, model, review_context).await,
        }
    }
}

#[derive(Serialize)]
struct CheckCacheParams<'a> {
    target: CheckTarget,
    review_context: &'a ReviewContextDigest,
}

#[derive(Deserialize, Serialize)]
struct CachedCheckResult {
    summary: String,
    findings: Vec<Finding>,
    iterations: u32,
}

fn check_cache_key(
    check: &Check,
    provider: &str,
    model: &str,
    review_context: &ReviewContextDigest,
    console: Console,
) -> Option<CacheKey> {
    let params = CheckCacheParams {
        target: check.target(),
        review_context,
    };
    match CacheKey::from_params(format!("check-{}", check.name()), provider, model, &params) {
        Ok(key) => Some(key),
        Err(error) => {
            console.debug(format_args!("cannot build check cache key: {error:?}"));
            None
        }
    }
}

fn load_check_cache(
    store: &CacheStore,
    key: &CacheKey,
    check: &Check,
    model: &str,
    context_usage: Option<LlmUsage>,
    console: Console,
) -> Option<CheckResult> {
    let cached = match store.read_json::<CachedCheckResult>(key) {
        Ok(Some(cached)) => cached,
        Ok(None) => return None,
        Err(error) => {
            console.debug(format_args!("ignoring check cache read error: {error:?}"));
            return None;
        }
    };
    if !cached.findings.iter().all(|finding| {
        check
            .expected_commits()
            .iter()
            .any(|expected| expected.matches(&finding.commit))
    }) {
        console.debug(format_args!(
            "ignoring cached check result with finding outside the current target"
        ));
        return None;
    }

    Some(CheckResult {
        check: check.name().to_string(),
        target: check.target(),
        ordered_commits: check.expected_commits().to_vec(),
        summary: cached.summary,
        findings: cached.findings,
        iterations: cached.iterations,
        error: None,
        context_usage,
        usage: LlmUsage {
            input_tokens: 0,
            output_tokens: 0,
            cost_usd: 0.0,
            model: model.to_string(),
        },
    })
}

fn store_check_cache(store: &CacheStore, key: &CacheKey, result: &CheckResult, console: Console) {
    if result.error.is_some() {
        console.debug(format_args!("not caching incomplete check result"));
        return;
    }
    let cached = CachedCheckResult {
        summary: result.summary.clone(),
        findings: result.findings.clone(),
        iterations: result.iterations,
    };
    if let Err(error) = store.write_json(key, &cached) {
        console.debug(format_args!("ignoring check cache write error: {error:?}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cached_checks_do_not_require_provider_creation() {
        let directory = tempfile::tempdir().unwrap();
        let console = Console::default();
        crate::git::run_git(&["init"], directory.path(), console)
            .await
            .unwrap();
        crate::git::run_git(
            &["config", "user.email", "test@example.com"],
            directory.path(),
            console,
        )
        .await
        .unwrap();
        crate::git::run_git(
            &["config", "user.name", "Test User"],
            directory.path(),
            console,
        )
        .await
        .unwrap();
        crate::git::run_git(
            &["commit", "--allow-empty", "-m", "cached commit"],
            directory.path(),
            console,
        )
        .await
        .unwrap();

        let mut config: Config = toml::from_str(crate::config::DEFAULT_CONFIG_TOML).unwrap();
        let (provider_name, model_name) = {
            let (provider, model) = config.resolve_provider(None, None).unwrap();
            (provider.name.clone(), model.name.clone())
        };
        config
            .providers
            .iter_mut()
            .find(|provider| provider.name == provider_name)
            .unwrap()
            .api_key_env = "PEER_TEST_MISSING_CHECK_CACHE_API_KEY".to_string();
        let extractor = Extractor::new(directory.path().to_path_buf(), console);
        let check = Check::Size(SizeCheck::try_new("HEAD", &extractor).await.unwrap());
        let review_context = ReviewContextDigest::default();
        let cache_store = CacheStore::new(directory.path().join(".peer/cache"), console);
        let cache_key = check_cache_key(
            &check,
            &provider_name,
            &model_name,
            &review_context,
            console,
        )
        .unwrap();
        let cached = CheckResult {
            check: check.name().to_string(),
            target: check.target(),
            ordered_commits: check.expected_commits().to_vec(),
            summary: "cached result".to_string(),
            findings: Vec::new(),
            iterations: 1,
            error: None,
            context_usage: None,
            usage: LlmUsage {
                input_tokens: 1,
                output_tokens: 1,
                cost_usd: 1.0,
                model: model_name.clone(),
            },
        };
        store_check_cache(&cache_store, &cache_key, &cached, console);

        let result = handler(
            console,
            CheckCommand::Size {
                revision: "HEAD".to_string(),
            },
            &config,
            directory.path().to_path_buf(),
            &cache_store,
            &review_context,
            None,
        )
        .await
        .unwrap();

        assert_eq!(result.summary, "cached result");
        assert_eq!(result.usage.input_tokens, 0);
        assert_eq!(result.usage.output_tokens, 0);
    }
}
