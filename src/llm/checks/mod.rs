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
use crate::llm::provider::{
    ConversationTurn, LlmProvider, ProviderCreationError, ToolSpec, create_provider,
};
use crate::llm::result::{
    CheckOutput, CheckResult, CheckTarget, validate_per_commit_targets, validate_range_targets,
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
) -> Result<CheckResult, CheckCommandError> {
    match command {
        CheckCommand::Size { revision } => {
            run_definition(SizeCheck::new(revision), console, config, project_root).await
        }
        CheckCommand::Intent { revision } => {
            run_definition(IntentCheck::new(revision), console, config, project_root).await
        }
        CheckCommand::Quality { revision } => {
            run_definition(QualityCheck::new(revision), console, config, project_root).await
        }
        CheckCommand::Security { revision } => {
            run_definition(SecurityCheck::new(revision), console, config, project_root).await
        }
        CheckCommand::Coherence { range } => {
            run_definition(CoherenceCheck::new(range), console, config, project_root).await
        }
    }
}

async fn run_definition<C>(
    check: C,
    console: Console,
    config: &Config,
    project_root: PathBuf,
) -> Result<CheckResult, CheckCommandError>
where
    C: CheckDefinition,
{
    let (provider_config, _) =
        config.resolve_provider(&config.llm.default_provider, &config.llm.default_model)?;

    let provider = create_provider(
        &provider_config.name,
        &provider_config.api_key_env,
        provider_config.base_url.as_deref(),
    )?;
    let extractor = Extractor::new(project_root.clone(), console);
    let tool_executor = PeerToolExecutor::new(Extractor::new(project_root, console));

    run_definition_with(
        check,
        console,
        config,
        &extractor,
        &provider,
        &tool_executor,
    )
    .await
}

async fn run_definition_with<C, P, E>(
    check: C,
    console: Console,
    config: &Config,
    extractor: &Extractor,
    provider: &P,
    tool_executor: &E,
) -> Result<CheckResult, CheckCommandError>
where
    C: CheckDefinition,
    P: LlmProvider,
    E: ToolExecutor,
{
    let confidence_threshold = Confidence::try_from(config.llm.confidence_threshold)?;
    let max_iterations = config.llm.max_iterations;
    let (_, model_config) =
        config.resolve_provider(&config.llm.default_provider, &config.llm.default_model)?;
    let run_config = CheckRunConfig {
        model: &model_config.name,
        confidence_threshold,
        max_iterations,
        input_per_1m_usd: model_config.input_per_1m_usd,
        output_per_1m_usd: model_config.output_per_1m_usd,
        console,
    };

    Ok(run_check(&check, extractor, provider, tool_executor, run_config).await?)
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

    /// Loads required data and builds the initial agent inputs.
    async fn prepare(&self, extractor: &Extractor) -> Result<PreparedCheck, ExtractError>;
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
    use crate::llm::agent::ToolExecutionResult;
    use crate::llm::confidence::Confidence;
    use crate::llm::provider::{LlmCallError, LlmCallResult, LlmResponse, RawUsage, ToolCall};
    use crate::llm::result::{Finding, Severity};
    use crate::llm::test_support::{FakeToolExecutor, MockProvider};

    struct TestCheck {
        target: CommitHash,
    }

    impl CheckDefinition for TestCheck {
        fn name(&self) -> &'static str {
            "test"
        }

        async fn prepare(&self, _extractor: &Extractor) -> Result<PreparedCheck, ExtractError> {
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

    async fn run_with_injected_dependencies<C>(check: C) -> (CheckResult, MockProvider)
    where
        C: CheckDefinition,
    {
        let repository = init_repository().await;
        let console = Console::default();
        let provider = successful_provider();
        let tool_executor = FakeToolExecutor::new(Vec::<ToolExecutionResult>::new());
        let extractor = Extractor::new(repository.path().to_path_buf(), console);

        let result = run_definition_with(
            check,
            console,
            &test_config(),
            &extractor,
            &provider,
            &tool_executor,
        )
        .await
        .unwrap();

        (result, provider)
    }

    #[tokio::test]
    async fn runs_size_check_with_injected_dependencies() {
        let (result, provider) =
            run_with_injected_dependencies(SizeCheck::new("HEAD".to_string())).await;

        assert_eq!(result.check, "size");
        assert_eq!(result.summary, "done");
        assert_eq!(result.iterations, 1);
        assert!(!result.is_exhausted);
        assert_eq!(provider.requests().len(), 1);
    }

    #[tokio::test]
    async fn runs_intent_check_with_injected_dependencies() {
        let (result, _) =
            run_with_injected_dependencies(IntentCheck::new("HEAD".to_string())).await;

        assert_eq!(result.check, "intent");
    }

    #[tokio::test]
    async fn runs_quality_check_with_injected_dependencies() {
        let (result, _) =
            run_with_injected_dependencies(QualityCheck::new("HEAD".to_string())).await;

        assert_eq!(result.check, "quality");
    }

    #[tokio::test]
    async fn runs_security_check_with_injected_dependencies() {
        let (result, _) =
            run_with_injected_dependencies(SecurityCheck::new("HEAD".to_string())).await;

        assert_eq!(result.check, "security");
    }

    #[tokio::test]
    async fn runs_coherence_check_with_injected_dependencies() {
        let (result, _) =
            run_with_injected_dependencies(CoherenceCheck::new("HEAD~1..HEAD".to_string())).await;

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

        let result = run_definition_with(
            IntentCheck::new("HEAD".to_string()),
            console,
            &test_config(),
            &extractor,
            &provider,
            &tool_executor,
        )
        .await
        .unwrap();

        assert_eq!(result.iterations, 2);
        assert_eq!(tool_executor.calls(), vec![tool_call]);
        assert_eq!(provider.requests().len(), 2);
    }

    async fn run_with_provider_error(error: LlmCallError) -> CheckCommandError {
        let repository = init_repository().await;
        let console = Console::default();
        let provider = MockProvider::new([Err(error)]);
        let tool_executor = FakeToolExecutor::new(Vec::<ToolExecutionResult>::new());
        let extractor = Extractor::new(repository.path().to_path_buf(), console);

        run_definition_with(
            SizeCheck::new("HEAD".to_string()),
            console,
            &test_config(),
            &extractor,
            &provider,
            &tool_executor,
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
        let tool_executor = FakeToolExecutor::new(Vec::<ToolExecutionResult>::new());
        let extractor = Extractor::new(repository.path().to_path_buf(), console);
        let mut config = test_config();
        config.llm.max_iterations = 1;

        let result = run_definition_with(
            SecurityCheck::new("HEAD".to_string()),
            console,
            &config,
            &extractor,
            &provider,
            &tool_executor,
        )
        .await
        .unwrap();

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

        let prepared = check.prepare(&extractor).await.unwrap();
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
