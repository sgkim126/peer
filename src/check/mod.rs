mod coherence;
mod intent;
mod quality;
mod result;
mod runner;
mod security;
mod size;

pub use self::result::{CheckError, CheckResult, CheckTarget, Finding, Severity};

#[cfg(test)]
pub use self::result::FileLocation;

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
use crate::llm::LlmUsage;
use crate::pi::{ModelRef, ModelRefError, PiRuntime, ReadTool, TerminalTool};

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
    async fn request(
        &self,
        extractor: &Extractor,
        review_context: &ReviewContextDigest,
    ) -> Result<CheckRequest, ExtractError>;
}

struct CheckRequest {
    system_prompt: String,
    prompt: String,
    read_tools: Vec<ReadTool>,
    terminal_tools: Vec<TerminalTool>,
}

impl CheckRequest {
    fn new(
        system_prompt: &str,
        prompt: String,
        read_tools: Vec<ReadTool>,
        review_context: &ReviewContextDigest,
    ) -> Self {
        let prompt = match review_context.to_prompt() {
            Some(context) => format!("{prompt}\n\n{context}"),
            None => prompt,
        };
        Self {
            system_prompt: system_prompt.to_string(),
            prompt,
            read_tools,
            terminal_tools: vec![
                TerminalTool::RequestClarification,
                TerminalTool::SubmitCheckResult,
            ],
        }
    }
}

#[derive(Debug)]
pub enum CheckCommandError {
    Config(crate::error::PeerError),
    Run(CheckRunError),
}

impl fmt::Display for CheckCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => error.fmt(f),
            Self::Run(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for CheckCommandError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Config(error) => Some(error),
            Self::Run(error) => Some(error),
        }
    }
}

impl From<crate::error::PeerError> for CheckCommandError {
    fn from(error: crate::error::PeerError) -> Self {
        Self::Config(error)
    }
}

impl From<CheckRunError> for CheckCommandError {
    fn from(error: CheckRunError) -> Self {
        Self::Run(error)
    }
}

impl From<ModelRefError> for CheckCommandError {
    fn from(error: ModelRefError) -> Self {
        Self::Run(error.into())
    }
}

impl From<ExtractError> for CheckCommandError {
    fn from(error: ExtractError) -> Self {
        Self::Run(error.into())
    }
}

pub struct CheckOptions<'a> {
    pub context_usage: Option<LlmUsage>,
    pub resume: bool,
    pub review_head: CommitHash,
    pub runtime: &'a mut PiRuntime,
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
    options: CheckOptions<'_>,
) -> Result<CheckResult, CheckCommandError> {
    let CheckOptions {
        context_usage,
        resume,
        review_head,
        runtime,
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
    let provider = &config.llm.default_provider;
    let model_name = &config.llm.default_model;
    let max_iterations = config.max_iterations_for(check.name()).get();
    let cache_key = check_cache_key(&check, provider, model_name, review_context, console);
    let cached = cache_key.as_ref().and_then(|key| {
        load_check_cache(
            cache_store,
            key,
            &check,
            model_name,
            context_usage.clone(),
            console,
        )
    });
    if let Some(LoadedCheckCache::Complete(result)) = cached {
        return Ok(*result);
    }
    let session_key = check_session_key(&check, provider, model_name, review_context)?;
    let model = ModelRef::try_new(provider.as_str(), model_name.as_str())?;
    let result = Checker::new(
        extractor,
        CheckRunConfig {
            model,
            max_iterations,
            context_usage,
            session_key,
            resume,
            console,
        },
    )
    .run(runtime, &check, review_context)
    .await?;
    if let Some(key) = &cache_key {
        update_check_cache(cache_store, key, &result, console);
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

    async fn request(
        &self,
        extractor: &Extractor,
        review_context: &ReviewContextDigest,
    ) -> Result<CheckRequest, ExtractError> {
        match self {
            Self::Size(check) => check.request(extractor, review_context).await,
            Self::Intent(check) => check.request(extractor, review_context).await,
            Self::Quality(check) => check.request(extractor, review_context).await,
            Self::Security(check) => check.request(extractor, review_context).await,
            Self::Coherence(check) => check.request(extractor, review_context).await,
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
}

enum LoadedCheckCache {
    Complete(Box<CheckResult>),
}

fn check_cache_params<'a>(
    check: &'a Check,
    review_context: &'a ReviewContextDigest,
) -> CheckCacheParams<'a> {
    CheckCacheParams {
        target: check.target(),
        review_head: match check {
            Check::Quality(check) => Some(check.review_head()),
            _ => None,
        },
        review_context,
    }
}

fn check_cache_key(
    check: &Check,
    provider: &str,
    model: &str,
    review_context: &ReviewContextDigest,
    console: Console,
) -> Option<CacheKey> {
    let params = check_cache_params(check, review_context);
    match CacheKey::from_params(format!("check-{}", check.name()), provider, model, &params) {
        Ok(key) => Some(key),
        Err(error) => {
            console.debug(format_args!("cannot build check cache key: {error:?}"));
            None
        }
    }
}

fn check_session_key(
    check: &Check,
    provider: &str,
    model: &str,
    review_context: &ReviewContextDigest,
) -> Result<CacheKey, CheckRunError> {
    Ok(CacheKey::from_params(
        format!("pi-session-check-{}", check.name()),
        provider,
        model,
        &check_cache_params(check, review_context),
    )?)
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
    let CachedCheck::Complete {
        summary,
        findings,
        iterations,
    } = cached;
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

fn update_check_cache(store: &CacheStore, key: &CacheKey, result: &CheckResult, console: Console) {
    if result.error.is_some() {
        console.debug(format_args!("not caching incomplete check result"));
        return;
    }
    let cached = CachedCheck::Complete {
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

    use std::assert_matches;

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
    fn cache_update_stores_only_complete_results() {
        let directory = tempfile::tempdir().unwrap();
        let console = Console::default();
        let cache_store = CacheStore::new(directory.path(), console);
        let cache_key =
            CacheKey::from_params("check-size", "test", "test-model", &"abc1234").unwrap();
        let incomplete = result(Some(CheckError::Exhausted {
            reason: "exhausted".to_string(),
        }));
        update_check_cache(&cache_store, &cache_key, &incomplete, console);
        assert_matches!(cache_store.read_json::<CachedCheck>(&cache_key), Ok(None),);

        update_check_cache(&cache_store, &cache_key, &result(None), console);
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

    #[tokio::test]
    async fn cached_checks_do_not_start_pi() {
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

        let config: Config = toml::from_str(crate::config::DEFAULT_CONFIG_TOML).unwrap();
        let provider_name = config.llm.default_provider.clone();
        let model_name = config.llm.default_model.clone();
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

        let mut runtime = PiRuntime::new(directory.path(), cache_store.clone(), console);
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
                runtime: &mut runtime,
            },
        )
        .await
        .unwrap();

        assert_eq!(result.summary, "cached result");
        assert_eq!(result.usage.input_tokens, 0);
        assert_eq!(result.usage.output_tokens, 0);
    }
}
