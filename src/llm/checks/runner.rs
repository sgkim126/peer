use std::fmt;

use crate::console::Console;
use crate::extract::{ExtractError, Extractor};
use crate::llm::agent::{AgentRequest, ToolExecutor, run_agent};
use crate::llm::confidence::Confidence;
use crate::llm::provider::{LlmCallError, LlmProvider};
use crate::llm::result::{CheckOutput, CheckResult};

use super::CheckDefinition;

pub struct CheckRunConfig<'a> {
    pub model: &'a str,
    pub confidence_threshold: Confidence,
    pub max_iterations: u32,
    pub input_per_1m_usd: f64,
    pub output_per_1m_usd: f64,
    pub console: Console,
}

#[derive(Debug)]
pub enum CheckRunError {
    Preparation(ExtractError),
    LlmCall(LlmCallError),
}

impl fmt::Display for CheckRunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Preparation(error) => write!(f, "failed to prepare check: {error}"),
            Self::LlmCall(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for CheckRunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Preparation(error) => Some(error),
            Self::LlmCall(error) => Some(error),
        }
    }
}

impl From<ExtractError> for CheckRunError {
    fn from(error: ExtractError) -> Self {
        CheckRunError::Preparation(error)
    }
}

impl From<LlmCallError> for CheckRunError {
    fn from(error: LlmCallError) -> Self {
        CheckRunError::LlmCall(error)
    }
}

pub async fn run_check<C, P, E>(
    check: &C,
    extractor: &Extractor,
    provider: &P,
    tool_executor: &E,
    config: CheckRunConfig<'_>,
) -> Result<CheckResult, CheckRunError>
where
    C: CheckDefinition,
    P: LlmProvider,
    E: ToolExecutor,
{
    let prepared = check.prepare(extractor).await?;
    let validate_output = |output: &CheckOutput| prepared.validate_output(output);
    let outcome = run_agent(
        provider,
        tool_executor,
        AgentRequest {
            model: config.model,
            conversation: &prepared.conversation,
            tools: &prepared.tools,
            output_schema: &prepared.output_schema,
            validate_output: &validate_output,
            confidence_threshold: config.confidence_threshold,
            max_iterations: config.max_iterations,
            console: config.console,
        },
    )
    .await?;

    Ok(CheckResult::from_agent_outcome(
        check.name().to_string(),
        prepared.result_target(),
        outcome,
        config.model.to_string(),
        config.input_per_1m_usd,
        config.output_per_1m_usd,
    ))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;

    use super::*;
    use crate::git::CommitHash;
    use crate::llm::agent::ToolExecutionResult;
    use crate::llm::checks::{PreparedCheck, PreparedCheckTarget};
    use crate::llm::provider::{ConversationTurn, LlmCallResult, LlmResponse, RawUsage};
    use crate::llm::result::{CheckOutput, Finding, Severity};
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
                conversation: vec![ConversationTurn::System("review the commit".to_string())],
                tools: Vec::new(),
                output_schema: json!({ "type": "object" }),
                target: PreparedCheckTarget::Commit(self.target.clone()),
            })
        }
    }

    fn output(commit: &str, confidence: f64) -> CheckOutput {
        CheckOutput {
            summary: "summary".to_string(),
            findings: vec![Finding {
                commit: CommitHash::new(commit).unwrap(),
                severity: Severity::Medium,
                message: "finding".to_string(),
                location: None,
            }],
            confidence: Confidence::try_from(confidence).unwrap(),
        }
    }

    fn response(output: CheckOutput) -> LlmCallResult {
        LlmCallResult {
            response: LlmResponse::CheckOutput(output),
            usage: RawUsage {
                input_tokens: 1_000,
                output_tokens: 500,
            },
        }
    }

    fn config() -> CheckRunConfig<'static> {
        CheckRunConfig {
            model: "test-model",
            confidence_threshold: Confidence::try_from(0.8).unwrap(),
            max_iterations: 2,
            input_per_1m_usd: 2.0,
            output_per_1m_usd: 6.0,
            console: Console::default(),
        }
    }

    fn extractor() -> Extractor {
        Extractor::new(PathBuf::from("/unused"), Console::default())
    }

    #[tokio::test]
    async fn runs_prepared_check_and_builds_check_result() {
        let provider = MockProvider::new([Ok(response(output("abc1234", 0.9)))]);
        let executor = FakeToolExecutor::new(Vec::<ToolExecutionResult>::new());
        let check = TestCheck {
            target: CommitHash::new("abc1234").unwrap(),
        };

        let result = run_check(&check, &extractor(), &provider, &executor, config())
            .await
            .unwrap();

        assert_eq!(result.check, "test");
        assert_eq!(result.summary, "summary");
        assert_eq!(result.iterations, 1);
        assert!(!result.is_exhausted);
        assert_eq!(result.exhaustion_reason, None);
        assert_eq!(result.usage.model, "test-model");
        assert_eq!(result.usage.cost_usd, 0.005);
        assert_eq!(
            provider.requests()[0].conversation,
            [ConversationTurn::System("review the commit".to_string())]
        );
    }

    #[tokio::test]
    async fn preserves_exhausted_result_metadata() {
        let provider = MockProvider::new([
            Ok(response(output("abc1234", 0.7))),
            Err(LlmCallError::ContextOverflow {
                message: "context is full".to_string(),
            }),
        ]);
        let executor = FakeToolExecutor::new(Vec::<ToolExecutionResult>::new());
        let check = TestCheck {
            target: CommitHash::new("abc1234").unwrap(),
        };

        let result = run_check(&check, &extractor(), &provider, &executor, config())
            .await
            .unwrap();

        assert!(result.is_exhausted);
        assert_eq!(result.confidence.as_f64(), 0.7);
        assert_eq!(
            result.exhaustion_reason.as_deref(),
            Some("LLM context length exceeded: context is full")
        );
        assert_eq!(result.iterations, 2);
    }

    #[tokio::test]
    async fn retries_output_for_the_wrong_target() {
        let provider = MockProvider::new([
            Ok(response(output("def5678", 0.9))),
            Ok(response(output("abc1234", 0.9))),
        ]);
        let executor = FakeToolExecutor::new(Vec::<ToolExecutionResult>::new());
        let check = TestCheck {
            target: CommitHash::new("abc1234").unwrap(),
        };

        let result = run_check(&check, &extractor(), &provider, &executor, config())
            .await
            .unwrap();

        assert_eq!(
            result.findings[0].commit,
            CommitHash::new("abc1234").unwrap()
        );
        assert_eq!(result.iterations, 2);
    }
}
