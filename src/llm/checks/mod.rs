mod coherence;
mod intent;
mod output;
mod quality;
pub mod runner;
mod security;
mod size;

pub use output::{CheckCommandErrorOutput, CheckCommandOutput, ErrorCode};

use std::fmt;
use std::path::PathBuf;

use crate::cache::{CacheKey, CacheStore};
use crate::cli::CheckCommand;
use crate::config::Config;
use crate::console::Console;
use crate::extract::{ExtractError, Extractor};
use crate::git::CommitHash;
use crate::llm::agent::ToolExecutor;
use crate::llm::checks::coherence::CoherenceCheck;
use crate::llm::checks::intent::IntentCheck;
use crate::llm::checks::quality::QualityCheck;
use crate::llm::checks::runner::{CheckRunConfig, CheckRunError, run_check};
use crate::llm::checks::security::SecurityCheck;
use crate::llm::checks::size::SizeCheck;
use crate::llm::confidence::{Confidence, ConfidenceError};
use crate::llm::context::ReviewContext;
use crate::llm::provider::{
    ConversationTurn, LlmProvider, ProviderCreationError, ToolSpec, create_provider,
};
use crate::llm::result::{
    CheckOutcome, CheckOutput, CheckTarget, validate_per_commit_targets, validate_range_targets,
};
use crate::llm::tool_executor::PeerToolExecutor;

#[derive(Debug)]
pub enum CheckCommandError {
    Config(crate::error::PeerError),
    InvalidConfidence(ConfidenceError),
    Provider(ProviderCreationError),
    Run(CheckRunError),
}

impl From<crate::error::PeerError> for CheckCommandError {
    fn from(err: crate::error::PeerError) -> Self {
        Self::Config(err)
    }
}

impl From<ConfidenceError> for CheckCommandError {
    fn from(err: ConfidenceError) -> Self {
        Self::InvalidConfidence(err)
    }
}

impl From<ProviderCreationError> for CheckCommandError {
    fn from(err: ProviderCreationError) -> Self {
        Self::Provider(err)
    }
}

impl From<CheckRunError> for CheckCommandError {
    fn from(err: CheckRunError) -> Self {
        Self::Run(err)
    }
}

impl From<ExtractError> for CheckCommandError {
    fn from(err: ExtractError) -> Self {
        Self::Run(CheckRunError::Preparation(err))
    }
}

impl fmt::Display for CheckCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => error.fmt(f),
            Self::InvalidConfidence(error) => error.fmt(f),
            Self::Provider(error) => error.fmt(f),
            Self::Run(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for CheckCommandError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Config(error) => Some(error),
            Self::InvalidConfidence(error) => Some(error),
            Self::Provider(error) => Some(error),
            Self::Run(error) => Some(error),
        }
    }
}

pub async fn handler(
    console: Console,
    command: CheckCommand,
    config: &Config,
    project_root: PathBuf,
    review_context: &ReviewContext,
) -> Result<CheckOutcome, CheckCommandError> {
    let (provider_config, _) =
        config.resolve_provider(&config.llm.default_provider, &config.llm.default_model)?;
    let provider_name = provider_config.name.clone();

    let provider = create_provider(
        &provider_config.name,
        &provider_config.api_key_env,
        provider_config.base_url.as_deref(),
        console,
    )?;
    let extractor = Extractor::new(project_root.clone(), console);
    let cache_store = CacheStore::new(project_root.join(".peer/cache"), console);
    let tool_executor = PeerToolExecutor::new(Extractor::new(project_root, console));

    let check = match command {
        CheckCommand::Size { revision } => {
            ResolvedCheck::Size(SizeCheck::try_new(&revision, &extractor).await?)
        }
        CheckCommand::Intent { revision } => {
            ResolvedCheck::Intent(IntentCheck::try_new(&revision, &extractor).await?)
        }
        CheckCommand::Quality { revision } => {
            ResolvedCheck::Quality(QualityCheck::try_new(&revision, &extractor).await?)
        }
        CheckCommand::Security { revision } => {
            ResolvedCheck::Security(SecurityCheck::try_new(&revision, &extractor).await?)
        }
        CheckCommand::Coherence { range } => {
            ResolvedCheck::Coherence(CoherenceCheck::try_new(&range, &extractor).await?)
        }
    };

    run_definition_with(
        check,
        CheckExecution {
            console,
            config,
            provider_name: &provider_name,
            cache: Some(&cache_store),
            extractor: &extractor,
            provider: &provider,
            tool_executor: &tool_executor,
            review_context,
        },
    )
    .await
}

struct CheckExecution<'a, P, E> {
    console: Console,
    config: &'a Config,
    provider_name: &'a str,
    cache: Option<&'a CacheStore>,
    extractor: &'a Extractor,
    provider: &'a P,
    tool_executor: &'a E,
    review_context: &'a ReviewContext,
}

async fn run_definition_with<C, P, E>(
    check: C,
    execution: CheckExecution<'_, P, E>,
) -> Result<CheckOutcome, CheckCommandError>
where
    C: CheckDefinition,
    P: LlmProvider,
    E: ToolExecutor,
{
    let confidence_threshold = Confidence::try_from(execution.config.llm.confidence_threshold)?;
    let max_iterations = execution.config.llm.max_iterations;
    let (_, model_config) = execution.config.resolve_provider(
        &execution.config.llm.default_provider,
        &execution.config.llm.default_model,
    )?;
    let run_config = CheckRunConfig {
        provider: execution.provider_name,
        model: &model_config.name,
        confidence_threshold,
        max_iterations,
        input_per_1m_usd: model_config.input_per_1m_usd,
        output_per_1m_usd: model_config.output_per_1m_usd,
        console: execution.console,
        cache: execution.cache,
    };

    Ok(run_check(
        &check,
        execution.extractor,
        execution.provider,
        execution.tool_executor,
        run_config,
        execution.review_context,
    )
    .await?)
}

/// Inputs prepared before the agent loop starts.
pub struct PreparedCheck {
    pub conversation: Vec<ConversationTurn>,
    pub tools: Vec<ToolSpec>,
    pub output_schema: serde_json::Value,
    pub target: PreparedCheckTarget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreparedCheckTarget {
    Commit(CommitHash),
    Range {
        revision: String,
        commits: Vec<CommitHash>,
    },
}

impl PreparedCheck {
    pub fn result_target(&self) -> CheckTarget {
        match &self.target {
            PreparedCheckTarget::Commit(commit) => CheckTarget::Commit(commit.clone()),
            PreparedCheckTarget::Range { revision, .. } => CheckTarget::Range(revision.clone()),
        }
    }

    pub fn validate_output(&self, output: &CheckOutput) -> Result<(), String> {
        match &self.target {
            PreparedCheckTarget::Commit(commit) => {
                validate_per_commit_targets(&output.findings, commit)
            }
            PreparedCheckTarget::Range { commits, .. } => {
                validate_range_targets(&output.findings, commits)
            }
        }
    }
}

/// Defines the provider-neutral inputs and validation rules for an LLM check.
pub trait CheckDefinition {
    /// Returns the stable name written to `CheckResult::check`.
    fn name(&self) -> &'static str;

    /// Returns the stable cache key for this resolved check target.
    fn cache_key(&self, provider: &str, model: &str, review_context: &ReviewContext) -> CacheKey;

    /// Loads required data and builds the initial agent inputs.
    async fn prepare(
        &self,
        extractor: &Extractor,
        review_context: &ReviewContext,
    ) -> Result<PreparedCheck, ExtractError>;
}

enum ResolvedCheck {
    Size(SizeCheck),
    Intent(IntentCheck),
    Quality(QualityCheck),
    Security(SecurityCheck),
    Coherence(CoherenceCheck),
}

impl CheckDefinition for ResolvedCheck {
    fn name(&self) -> &'static str {
        match self {
            Self::Size(check) => check.name(),
            Self::Intent(check) => check.name(),
            Self::Quality(check) => check.name(),
            Self::Security(check) => check.name(),
            Self::Coherence(check) => check.name(),
        }
    }

    fn cache_key(&self, provider: &str, model: &str, review_context: &ReviewContext) -> CacheKey {
        match self {
            Self::Size(check) => check.cache_key(provider, model, review_context),
            Self::Intent(check) => check.cache_key(provider, model, review_context),
            Self::Quality(check) => check.cache_key(provider, model, review_context),
            Self::Security(check) => check.cache_key(provider, model, review_context),
            Self::Coherence(check) => check.cache_key(provider, model, review_context),
        }
    }

    async fn prepare(
        &self,
        extractor: &Extractor,
        review_context: &ReviewContext,
    ) -> Result<PreparedCheck, ExtractError> {
        match self {
            Self::Size(check) => check.prepare(extractor, review_context).await,
            Self::Intent(check) => check.prepare(extractor, review_context).await,
            Self::Quality(check) => check.prepare(extractor, review_context).await,
            Self::Security(check) => check.prepare(extractor, review_context).await,
            Self::Coherence(check) => check.prepare(extractor, review_context).await,
        }
    }
}

fn all_tools() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "get_commit_message".to_string(),
            description: "Returns the full commit message for a commit.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "revision": {
                        "type": "string",
                        "description": "Git revision resolving to a commit."
                    }
                },
                "required": ["revision"]
            }),
        },
        ToolSpec {
            name: "get_commit_diff".to_string(),
            description: "Returns the full unified diff for a commit.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "revision": {
                        "type": "string",
                        "description": "Git revision resolving to a commit."
                    }
                },
                "required": ["revision"]
            }),
        },
        ToolSpec {
            name: "get_changed_files".to_string(),
            description: "Returns the files changed in a commit.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "revision": {
                        "type": "string",
                        "description": "Git revision resolving to a commit."
                    }
                },
                "required": ["revision"]
            }),
        },
        ToolSpec {
            name: "get_commits_in_range".to_string(),
            description: "Returns commit hashes in a two-dot range, oldest to newest.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "range": {
                        "type": "string",
                        "description": "Git two-dot range."
                    }
                },
                "required": ["range"]
            }),
        },
        ToolSpec {
            name: "get_file_content".to_string(),
            description: "Returns a file's content at a commit.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "revision": {
                        "type": "string",
                        "description": "Git revision at which to read the file."
                    },
                    "path": {
                        "type": "string",
                        "description": "Repository-root-relative path."
                    }
                },
                "required": ["path", "revision"]
            }),
        },
        ToolSpec {
            name: "request_user_info".to_string(),
            description: "Stop the check and ask the user for information that is necessary to complete the check but is not available from the provided context or repository tools. Do not ask for information that can be obtained with the other available tools. Each question must include enough context to explain why the information is needed.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "questions": {
                        "type": "array",
                        "items": {
                            "type": "string"
                        },
                        "minItems": 1,
                        "description": "Questions for the user. Include the reason the information is needed in each question."
                    }
                },
                "required": ["questions"]
            }),
        },
    ]
}

fn output_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "summary": {
                "type": "string",
                "description": "One-sentence summary of the check result."
            },
            "findings": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "commit": {
                            "type": "string"
                        },
                        "severity": {
                            "type": "string",
                            "enum": ["info", "low", "medium", "high", "critical"]
                        },
                        "message": {
                            "type": "string"
                        },
                        "file": {
                            "type": "string"
                        },
                        "line": {
                            "type": "integer",
                            "minimum": 1
                        }
                    },
                    "required": ["commit", "severity", "message"]
                }
            },
            "confidence": {
                "type": "number",
                "minimum": 0.0,
                "maximum": 1.0
            }
        },
        "required": ["summary", "findings", "confidence"]
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use std::path::PathBuf;

    use crate::config::{LlmConfig, ModelConfig, ProviderConfig, ReviewConfig};
    use crate::console::Console;
    use crate::git::run_git;
    use crate::llm::confidence::Confidence;
    use crate::llm::provider::{LlmCallError, LlmCallResult, LlmResponse, RawUsage, ToolCall};
    use crate::llm::result::{CheckOutcome, CheckResult, Finding, Severity};
    use crate::llm::test_support::{FakeToolExecutor, MockProvider};

    struct TestCheck {
        target: CommitHash,
    }

    impl CheckDefinition for TestCheck {
        fn name(&self) -> &'static str {
            "test"
        }

        fn cache_key(
            &self,
            provider: &str,
            model: &str,
            review_context: &ReviewContext,
        ) -> CacheKey {
            let params = TestCheckCacheParams {
                commit: &self.target,
                review_context,
            };

            CacheKey::from_params(self.name(), provider, model, &params)
                .expect("serializing test check cache params cannot fail")
        }

        async fn prepare(
            &self,
            _extractor: &Extractor,
            _review_context: &ReviewContext,
        ) -> Result<PreparedCheck, ExtractError> {
            Ok(PreparedCheck {
                conversation: vec![
                    ConversationTurn::System(format!("Review commit {}.", self.target)),
                    ConversationTurn::User("Commit message:\nAdd check preparation".to_string()),
                ],
                tools: vec![ToolSpec {
                    name: "get_commit_diff".to_string(),
                    description: "Read a commit diff.".to_string(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "hash": {
                                "type": "string"
                            }
                        },
                        "required": ["hash"]
                    }),
                }],
                output_schema: json!({
                    "type": "object"
                }),
                target: PreparedCheckTarget::Commit(self.target.clone()),
            })
        }
    }

    #[derive(Debug, serde::Serialize)]
    struct TestCheckCacheParams<'a> {
        commit: &'a CommitHash,
        review_context: &'a ReviewContext,
    }

    fn output(commit: &str) -> CheckOutput {
        CheckOutput {
            summary: "summary".to_string(),
            findings: vec![Finding {
                commit: CommitHash::new(commit).unwrap(),
                severity: Severity::Info,
                message: "message".to_string(),
                location: None,
            }],
            confidence: Confidence::try_from(0.9).unwrap(),
        }
    }

    fn test_config() -> Config {
        Config {
            version: 1,
            review: ReviewConfig { max_commits: 10 },
            llm: LlmConfig {
                default_provider: "test".to_string(),
                default_model: "test-model".to_string(),
                confidence_threshold: 0.8,
                max_iterations: 3,
            },
            providers: vec![ProviderConfig {
                name: "test".to_string(),
                api_key_env: "UNUSED_API_KEY".to_string(),
                base_url: None,
                models: vec![ModelConfig {
                    name: "test-model".to_string(),
                    input_per_1m_usd: 2.0,
                    output_per_1m_usd: 6.0,
                }],
            }],
        }
    }

    async fn init_repository() -> tempfile::TempDir {
        let repository = tempfile::tempdir().unwrap();
        let console = Console::default();
        run_git(&["init"], repository.path(), console)
            .await
            .unwrap();
        run_git(
            &["config", "user.email", "test@example.com"],
            repository.path(),
            console,
        )
        .await
        .unwrap();
        run_git(&["config", "user.name", "Test"], repository.path(), console)
            .await
            .unwrap();
        std::fs::write(repository.path().join("file.txt"), "content").unwrap();
        run_git(&["add", "file.txt"], repository.path(), console)
            .await
            .unwrap();
        run_git(
            &["commit", "--no-gpg-sign", "-m", "test commit"],
            repository.path(),
            console,
        )
        .await
        .unwrap();
        std::fs::write(repository.path().join("file.txt"), "updated content").unwrap();
        run_git(&["add", "file.txt"], repository.path(), console)
            .await
            .unwrap();
        run_git(
            &["commit", "--no-gpg-sign", "-m", "update test file"],
            repository.path(),
            console,
        )
        .await
        .unwrap();
        repository
    }

    fn successful_provider() -> MockProvider {
        MockProvider::new([Ok(LlmCallResult {
            response: LlmResponse::CheckOutput(CheckOutput {
                summary: "done".to_string(),
                findings: Vec::new(),
                confidence: Confidence::try_from(0.9).unwrap(),
            }),
            usage: RawUsage {
                input_tokens: 100,
                output_tokens: 50,
            },
        })])
    }

    fn success_result(outcome: &CheckOutcome) -> &CheckResult {
        match outcome {
            CheckOutcome::Success { check } => check,
            CheckOutcome::NeedsUserInfo { .. } => panic!("expected successful check outcome"),
        }
    }

    async fn run_check_with_repository<C>(
        repository: &tempfile::TempDir,
        check: C,
    ) -> (CheckResult, MockProvider)
    where
        C: CheckDefinition,
    {
        let console = Console::default();
        let provider = successful_provider();
        let tool_executor = FakeToolExecutor::default();
        let extractor = Extractor::new(repository.path().to_path_buf(), console);

        let outcome = run_definition_with(
            check,
            CheckExecution {
                console,
                config: &test_config(),
                provider_name: "test",
                cache: None,
                extractor: &extractor,
                provider: &provider,
                tool_executor: &tool_executor,
                review_context: &ReviewContext::default(),
            },
        )
        .await
        .unwrap();
        let result = success_result(&outcome).clone();

        (result, provider)
    }

    #[tokio::test]
    async fn runs_size_check_with_injected_dependencies() {
        let repository = init_repository().await;
        let extractor = Extractor::new(repository.path().to_path_buf(), Console::default());
        let check = SizeCheck::try_new("HEAD", &extractor).await.unwrap();
        let (result, provider) = run_check_with_repository(&repository, check).await;

        assert_eq!(result.check, "size");
        assert_eq!(result.summary, "done");
        assert_eq!(result.iterations, 1);
        assert!(!result.is_exhausted);
        assert_eq!(provider.requests().len(), 1);
    }

    #[tokio::test]
    async fn runs_intent_check_with_injected_dependencies() {
        let repository = init_repository().await;
        let extractor = Extractor::new(repository.path().to_path_buf(), Console::default());
        let check = IntentCheck::try_new("HEAD", &extractor).await.unwrap();
        let (result, _) = run_check_with_repository(&repository, check).await;

        assert_eq!(result.check, "intent");
    }

    #[tokio::test]
    async fn runs_quality_check_with_injected_dependencies() {
        let repository = init_repository().await;
        let extractor = Extractor::new(repository.path().to_path_buf(), Console::default());
        let check = QualityCheck::try_new("HEAD", &extractor).await.unwrap();
        let (result, _) = run_check_with_repository(&repository, check).await;

        assert_eq!(result.check, "quality");
    }

    #[tokio::test]
    async fn runs_security_check_with_injected_dependencies() {
        let repository = init_repository().await;
        let extractor = Extractor::new(repository.path().to_path_buf(), Console::default());
        let check = SecurityCheck::try_new("HEAD", &extractor).await.unwrap();
        let (result, _) = run_check_with_repository(&repository, check).await;

        assert_eq!(result.check, "security");
    }

    #[tokio::test]
    async fn runs_coherence_check_with_injected_dependencies() {
        let repository = init_repository().await;
        let extractor = Extractor::new(repository.path().to_path_buf(), Console::default());
        let check = CoherenceCheck::try_new("HEAD~1..HEAD", &extractor)
            .await
            .unwrap();
        let (result, _) = run_check_with_repository(&repository, check).await;

        assert_eq!(result.check, "coherence");
    }

    #[tokio::test]
    async fn runs_check_after_tool_call() {
        let repository = init_repository().await;
        let console = Console::default();
        let tool_call = ToolCall {
            id: "call-1".to_string(),
            name: "get_commit_diff".to_string(),
            arguments: json!({
                "revision": "HEAD"
            }),
        };
        let provider = MockProvider::new([
            Ok(LlmCallResult {
                response: LlmResponse::ToolCalls(vec![tool_call.clone()]),
                usage: RawUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                },
            }),
            Ok(LlmCallResult {
                response: LlmResponse::CheckOutput(CheckOutput {
                    summary: "done".to_string(),
                    findings: Vec::new(),
                    confidence: Confidence::try_from(0.9).unwrap(),
                }),
                usage: RawUsage {
                    input_tokens: 20,
                    output_tokens: 10,
                },
            }),
        ]);
        let tool_executor = FakeToolExecutor::new([Ok(json!("diff"))]);
        let extractor = Extractor::new(repository.path().to_path_buf(), console);
        let check = IntentCheck::try_new("HEAD", &extractor).await.unwrap();

        let outcome = run_definition_with(
            check,
            CheckExecution {
                console,
                config: &test_config(),
                provider_name: "test",
                cache: None,
                extractor: &extractor,
                provider: &provider,
                tool_executor: &tool_executor,
                review_context: &ReviewContext::default(),
            },
        )
        .await
        .unwrap();
        let result = success_result(&outcome);

        assert_eq!(result.iterations, 2);
        assert_eq!(tool_executor.calls(), vec![tool_call]);
        assert_eq!(provider.requests().len(), 2);
    }

    async fn run_with_provider_error(error: LlmCallError) -> CheckCommandError {
        let repository = init_repository().await;
        let console = Console::default();
        let provider = MockProvider::new([Err(error)]);
        let tool_executor = FakeToolExecutor::default();
        let extractor = Extractor::new(repository.path().to_path_buf(), console);
        let check = SizeCheck::try_new("HEAD", &extractor).await.unwrap();

        run_definition_with(
            check,
            CheckExecution {
                console,
                config: &test_config(),
                provider_name: "test",
                cache: None,
                extractor: &extractor,
                provider: &provider,
                tool_executor: &tool_executor,
                review_context: &ReviewContext::default(),
            },
        )
        .await
        .unwrap_err()
    }

    #[tokio::test]
    async fn propagates_transient_provider_failure() {
        let error = run_with_provider_error(LlmCallError::Transient {
            message: "request timed out".to_string(),
            source: Box::new(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "request timed out",
            )),
        })
        .await;

        assert!(matches!(
            error,
            CheckCommandError::Run(CheckRunError::LlmCall(LlmCallError::Transient { .. }))
        ));
    }

    #[tokio::test]
    async fn propagates_permanent_provider_failure() {
        let error = run_with_provider_error(LlmCallError::Permanent {
            message: "invalid request".to_string(),
            source: Box::new(std::io::Error::other("invalid request")),
        })
        .await;

        assert!(matches!(
            error,
            CheckCommandError::Run(CheckRunError::LlmCall(LlmCallError::Permanent { .. }))
        ));
    }

    #[tokio::test]
    async fn returns_exhausted_result_when_confidence_stays_below_threshold() {
        let repository = init_repository().await;
        let console = Console::default();
        let provider = MockProvider::new([Ok(LlmCallResult {
            response: LlmResponse::CheckOutput(CheckOutput {
                summary: "uncertain".to_string(),
                findings: Vec::new(),
                confidence: Confidence::try_from(0.7).unwrap(),
            }),
            usage: RawUsage {
                input_tokens: 100,
                output_tokens: 50,
            },
        })]);
        let tool_executor = FakeToolExecutor::default();
        let extractor = Extractor::new(repository.path().to_path_buf(), console);
        let mut config = test_config();
        config.llm.max_iterations = 1;
        let check = SecurityCheck::try_new("HEAD", &extractor).await.unwrap();

        let outcome = run_definition_with(
            check,
            CheckExecution {
                console,
                config: &config,
                provider_name: "test",
                cache: None,
                extractor: &extractor,
                provider: &provider,
                tool_executor: &tool_executor,
                review_context: &ReviewContext::default(),
            },
        )
        .await
        .unwrap();
        let result = success_result(&outcome);

        assert!(result.is_exhausted);
        assert_eq!(result.summary, "uncertain");
        assert_eq!(result.confidence.as_f64(), 0.7);
        assert_eq!(result.iterations, 1);
        assert_eq!(
            result.exhaustion_reason.as_deref(),
            Some("maximum iterations reached")
        );
    }

    #[tokio::test]
    async fn check_definition_prepares_required_inputs_before_agent_loop() {
        let target = CommitHash::new("abc1234").unwrap();
        let check = TestCheck {
            target: target.clone(),
        };
        let extractor = Extractor::new(PathBuf::from("/project"), Console::default());

        assert_eq!(check.name(), "test");

        let prepared = check
            .prepare(&extractor, &ReviewContext::default())
            .await
            .unwrap();
        assert_eq!(
            prepared.conversation,
            vec![
                ConversationTurn::System("Review commit abc1234.".to_string()),
                ConversationTurn::User("Commit message:\nAdd check preparation".to_string()),
            ]
        );
        assert_eq!(prepared.tools[0].name, "get_commit_diff");
        assert_eq!(
            prepared.output_schema,
            json!({
                "type": "object"
            })
        );
        assert_eq!(prepared.result_target(), CheckTarget::Commit(target));
    }

    #[test]
    fn prepared_check_owns_target_validation() {
        let prepared = PreparedCheck {
            conversation: Vec::new(),
            tools: Vec::new(),
            output_schema: json!({}),
            target: PreparedCheckTarget::Commit(CommitHash::new("abc1234").unwrap()),
        };

        assert!(prepared.validate_output(&output("abc1234")).is_ok());
        assert!(prepared.validate_output(&output("def5678")).is_err());
    }

    #[test]
    fn prepared_range_check_validates_against_loaded_commits() {
        let prepared = PreparedCheck {
            conversation: Vec::new(),
            tools: Vec::new(),
            output_schema: json!({}),
            target: PreparedCheckTarget::Range {
                revision: "HEAD~2..HEAD".to_string(),
                commits: vec![
                    CommitHash::new("abc1234").unwrap(),
                    CommitHash::new("def5678").unwrap(),
                ],
            },
        };

        assert!(prepared.validate_output(&output("def5678")).is_ok());
        assert!(prepared.validate_output(&output("9876abc")).is_err());
        assert_eq!(
            prepared.result_target(),
            CheckTarget::Range("HEAD~2..HEAD".to_string())
        );
    }
}
