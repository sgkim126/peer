mod coherence;
mod intent;
mod quality;
mod runner;
mod security;
mod size;

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cache::{CacheKey, CacheStore};
use crate::cli::CheckCommand;
use crate::config::Config;
use crate::console::Console;
use crate::context::ReviewContextDigest;
use crate::extract::{ExtractError, Extractor};
use crate::git::CommitHash;
use crate::llm::{
    AgentCheckpoint, AgentRequest, CheckResult, CheckTarget, Finding, LlmUsage,
    ProviderCreationError, ProviderRuntime,
};

use self::coherence::CoherenceCheck;
use self::intent::IntentCheck;
use self::quality::QualityCheck;
use self::runner::{CheckExecution, CheckRunConfig, CheckRunError, Checker};
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

pub struct CheckOptions {
    pub context_usage: Option<LlmUsage>,
    pub resume: bool,
    pub review_head: CommitHash,
}

pub async fn resolve_review_head(
    command: &CheckCommand,
    project_root: &Path,
    console: Console,
) -> Result<CommitHash, ExtractError> {
    let revision = match command {
        CheckCommand::Size { revision } => revision,
        CheckCommand::Intent { revision } => revision,
        CheckCommand::Quality { revision } => revision,
        CheckCommand::Security { revision } => revision,
        CheckCommand::Coherence { range } => {
            if range.contains("...") || !range.contains("..") {
                return Err(ExtractError::InvalidTwoDotRange(range.clone()));
            }
            let (from, to) = range.split_once("..").expect("range contains two dots");
            if from.is_empty() || to.is_empty() {
                return Err(ExtractError::InvalidTwoDotRange(range.clone()));
            }
            to
        }
    };
    Ok(CommitHash::resolve(revision, project_root, console).await?)
}

pub async fn handler(
    console: Console,
    command: CheckCommand,
    config: &Config,
    project_root: PathBuf,
    cache_store: &CacheStore,
    review_context: &ReviewContextDigest,
    options: CheckOptions,
) -> Result<CheckResult, CheckCommandError> {
    let CheckOptions {
        context_usage,
        resume,
        review_head,
    } = options;
    let extractor = Extractor::new(project_root, console);

    let check: Check = match command {
        CheckCommand::Size { revision } => {
            Check::Size(SizeCheck::try_new(&revision, &extractor).await?)
        }
        CheckCommand::Intent { revision } => {
            Check::Intent(IntentCheck::try_new(&revision, &extractor).await?)
        }
        CheckCommand::Quality { revision } => {
            Check::Quality(QualityCheck::try_new(&revision, review_head, &extractor).await?)
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
    let cached = cache_key.as_ref().and_then(|key| {
        load_check_cache(
            cache_store,
            key,
            &check,
            &model_config.name,
            context_usage.clone(),
            console,
        )
    });
    let checkpoint = match cached {
        Some(LoadedCheckCache::Complete(result)) => return Ok(*result),
        Some(LoadedCheckCache::Resumable(checkpoint)) if resume => Some(checkpoint),
        Some(LoadedCheckCache::Resumable(_)) => None,
        None => None,
    };
    let resumed = checkpoint.is_some();
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
            checkpoint,
            console,
        },
    )
    .run(&check, review_context)
    .await;
    let execution = match result {
        Ok(execution) => execution,
        Err(error) => {
            if !resumed {
                return Err(error.into());
            }
            if matches!(error, CheckRunError::Preparation(_)) {
                return Err(error.into());
            }
            if let Some(key) = &cache_key
                && let Err(error) = cache_store.remove(key)
            {
                console.debug(format_args!("ignoring check cache remove error: {error:?}"));
            }
            return Err(error.into());
        }
    };
    if let Some(key) = &cache_key {
        update_check_cache(cache_store, key, &execution, resumed, console);
    }
    Ok(execution.result)
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
    #[serde(skip_serializing_if = "Option::is_none")]
    review_head: Option<&'a CommitHash>,
    review_context: &'a ReviewContextDigest,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum CachedCheck {
    Complete {
        summary: String,
        findings: Vec<Finding>,
        iterations: u32,
    },
    Resumable {
        checkpoint: AgentCheckpoint,
    },
}

enum LoadedCheckCache {
    Complete(Box<CheckResult>),
    Resumable(AgentCheckpoint),
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
        review_head: match check {
            Check::Quality(check) => Some(check.review_head()),
            _ => None,
        },
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
) -> Option<LoadedCheckCache> {
    let cached = match store.read_json::<CachedCheck>(key) {
        Ok(Some(cached)) => cached,
        Ok(None) => return None,
        Err(error) => {
            console.debug(format_args!("ignoring check cache read error: {error:?}"));
            return None;
        }
    };
    let (summary, findings, iterations) = match cached {
        CachedCheck::Complete {
            summary,
            findings,
            iterations,
        } => (summary, findings, iterations),
        CachedCheck::Resumable { checkpoint } => {
            return Some(LoadedCheckCache::Resumable(checkpoint));
        }
    };
    if !findings.iter().all(|finding| {
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

    Some(LoadedCheckCache::Complete(Box::new(CheckResult {
        check: check.name().to_string(),
        target: check.target(),
        ordered_commits: check.expected_commits().to_vec(),
        summary,
        findings,
        iterations,
        error: None,
        context_usage,
        usage: LlmUsage {
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_usd: 0.0,
            model: model.to_string(),
            models: Vec::new(),
        },
    })))
}

fn update_check_cache(
    store: &CacheStore,
    key: &CacheKey,
    execution: &CheckExecution,
    resumed: bool,
    console: Console,
) {
    let cached = match &execution.checkpoint {
        Some(checkpoint) => CachedCheck::Resumable {
            checkpoint: checkpoint.clone(),
        },
        None if execution.result.error.is_none() => CachedCheck::Complete {
            summary: execution.result.summary.clone(),
            findings: execution.result.findings.clone(),
            iterations: execution.result.iterations,
        },
        None if resumed => {
            if let Err(error) = store.remove(key) {
                console.debug(format_args!("ignoring check cache remove error: {error:?}"));
            }
            return;
        }
        None => {
            console.debug(format_args!("not caching incomplete check result"));
            return;
        }
    };
    if let Err(error) = store.write_json(key, &cached) {
        console.debug(format_args!("ignoring check cache write error: {error:?}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::assert_matches;

    use crate::llm::{CheckError, ConversationTurn};

    fn result(error: Option<CheckError>) -> CheckResult {
        let commit = CommitHash::new("abc1234").unwrap();
        CheckResult {
            check: "size".to_string(),
            target: CheckTarget::Commit(commit.clone()),
            ordered_commits: vec![commit],
            summary: "cached result".to_string(),
            findings: Vec::new(),
            iterations: 2,
            error,
            context_usage: None,
            usage: LlmUsage {
                input_tokens: 1,
                output_tokens: 1,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: 1.0,
                model: "test-model".to_string(),
                models: Vec::new(),
            },
        }
    }

    #[test]
    fn quality_cache_key_depends_on_the_review_head() {
        let target = CheckTarget::Commit(CommitHash::new("abc1234").unwrap());
        let first_head = CommitHash::new("def5678").unwrap();
        let second_head = CommitHash::new("fed4321").unwrap();
        let review_context = ReviewContextDigest::default();
        let without_head = CacheKey::from_params(
            "check-quality",
            "test",
            "test-model",
            &CheckCacheParams {
                target: target.clone(),
                review_head: None,
                review_context: &review_context,
            },
        )
        .unwrap();
        let also_without_head = CacheKey::from_params(
            "check-quality",
            "test",
            "test-model",
            &CheckCacheParams {
                target: target.clone(),
                review_head: None,
                review_context: &review_context,
            },
        )
        .unwrap();
        let first = CacheKey::from_params(
            "check-quality",
            "test",
            "test-model",
            &CheckCacheParams {
                target: target.clone(),
                review_head: Some(&first_head),
                review_context: &review_context,
            },
        )
        .unwrap();
        let second = CacheKey::from_params(
            "check-quality",
            "test",
            "test-model",
            &CheckCacheParams {
                target,
                review_head: Some(&second_head),
                review_context: &review_context,
            },
        )
        .unwrap();

        assert_eq!(without_head.params_hash, also_without_head.params_hash);
        assert_ne!(without_head.params_hash, first.params_hash);
        assert_ne!(first.params_hash, second.params_hash);
    }

    #[test]
    fn cache_update_replaces_a_checkpoint_with_a_complete_result() {
        let directory = tempfile::tempdir().unwrap();
        let console = Console::default();
        let cache_store = CacheStore::new(directory.path(), console);
        let cache_key =
            CacheKey::from_params("check-size", "test", "test-model", &"abc1234").unwrap();
        let checkpoint = AgentCheckpoint {
            conversation: vec![ConversationTurn::System("Review code.".to_string())],
            iterations: 1,
        };
        update_check_cache(
            &cache_store,
            &cache_key,
            &CheckExecution {
                result: result(Some(CheckError::Exhausted {
                    reason: "exhausted".to_string(),
                })),
                checkpoint: Some(checkpoint.clone()),
            },
            false,
            console,
        );
        let cached = cache_store
            .read_json::<CachedCheck>(&cache_key)
            .unwrap()
            .unwrap();
        assert_matches!(
            cached,
            CachedCheck::Resumable {
                checkpoint: cached
            } if cached == checkpoint
        );

        update_check_cache(
            &cache_store,
            &cache_key,
            &CheckExecution {
                result: result(None),
                checkpoint: None,
            },
            true,
            console,
        );
        let cached = cache_store
            .read_json::<CachedCheck>(&cache_key)
            .unwrap()
            .unwrap();
        assert_matches!(
            cached,
            CachedCheck::Complete {
                summary,
                iterations: 2,
                ..
            } if summary == "cached result"
        );
    }

    #[test]
    fn non_resumable_failure_removes_a_loaded_checkpoint() {
        let directory = tempfile::tempdir().unwrap();
        let console = Console::default();
        let cache_store = CacheStore::new(directory.path(), console);
        let cache_key =
            CacheKey::from_params("check-size", "test", "test-model", &"abc1234").unwrap();
        cache_store
            .write_json(
                &cache_key,
                &CachedCheck::Resumable {
                    checkpoint: AgentCheckpoint {
                        conversation: Vec::new(),
                        iterations: 1,
                    },
                },
            )
            .unwrap();

        update_check_cache(
            &cache_store,
            &cache_key,
            &CheckExecution {
                result: result(Some(CheckError::Agent {
                    reason: "permanent failure".to_string(),
                })),
                checkpoint: None,
            },
            true,
            console,
        );

        assert!(
            cache_store
                .read_json::<CachedCheck>(&cache_key)
                .unwrap()
                .is_none()
        );
    }

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
        let checkpoint = AgentCheckpoint {
            conversation: vec![ConversationTurn::System("cached prompt".to_string())],
            iterations: 2,
        };
        cache_store
            .write_json(
                &cache_key,
                &CachedCheck::Resumable {
                    checkpoint: checkpoint.clone(),
                },
            )
            .unwrap();
        let Some(LoadedCheckCache::Resumable(loaded)) =
            load_check_cache(&cache_store, &cache_key, &check, &model_name, None, console)
        else {
            panic!("expected resumable check cache");
        };
        assert_eq!(loaded, checkpoint);

        cache_store
            .write_json(
                &cache_key,
                &CachedCheck::Complete {
                    summary: "cached result".to_string(),
                    findings: Vec::new(),
                    iterations: 1,
                },
            )
            .unwrap();

        let result = handler(
            console,
            CheckCommand::Size {
                revision: "HEAD".to_string(),
            },
            &config,
            directory.path().to_path_buf(),
            &cache_store,
            &review_context,
            CheckOptions {
                context_usage: None,
                resume: true,
                review_head: CommitHash::resolve("HEAD", directory.path(), console)
                    .await
                    .unwrap(),
            },
        )
        .await
        .unwrap();

        assert_eq!(result.summary, "cached result");
        assert_eq!(result.usage.input_tokens, 0);
        assert_eq!(result.usage.output_tokens, 0);
    }
}
