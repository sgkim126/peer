use std::fmt;

use crate::cache::CacheStore;
use crate::console::Console;
use crate::extract::{ExtractError, Extractor};
use crate::llm::agent::{AgentRequest, AgentRunOutcome, ToolExecutor, run_agent};
use crate::llm::context::ReviewContext;
use crate::llm::provider::{LlmCallError, LlmProvider};
use crate::llm::result::{CheckOutcome, CheckOutput, CheckResult, CheckUsage};

use super::CheckDefinition;

pub struct CheckRunConfig<'a> {
    pub provider: &'a str,
    pub model: &'a str,
    pub max_iterations: u32,
    pub input_per_1m_usd: f64,
    pub output_per_1m_usd: f64,
    pub console: Console,
    pub cache: Option<&'a CacheStore>,
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
    review_context: &ReviewContext,
) -> Result<CheckOutcome, CheckRunError>
where
    C: CheckDefinition,
    P: LlmProvider,
    E: ToolExecutor,
{
    let cache_key = check.cache_key(config.provider, config.model, review_context);
    if let Some(cache) = config.cache
        && let Ok(Some(mut result)) = cache.read_json::<CheckResult>(&cache_key)
    {
        result.usage = CheckUsage {
            input_tokens: 0,
            output_tokens: 0,
            cost_usd: 0.0,
            model: config.model.to_string(),
        };
        return Ok(CheckOutcome::success(result));
    }

    let prepared = check.prepare(extractor, review_context).await?;
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
            max_iterations: config.max_iterations,
            console: config.console,
        },
    )
    .await?;

    match outcome {
        AgentRunOutcome::NeedsUserInfo {
            request,
            usage,
            iterations,
        } => Ok(CheckOutcome::NeedsUserInfo {
            request: crate::llm::result::CheckUserInfoRequest {
                check: check.name().to_string(),
                target: prepared.result_target(),
                questions: request.questions,
                iterations,
                usage: CheckUsage::from_raw_usage(
                    usage,
                    config.model,
                    config.input_per_1m_usd,
                    config.output_per_1m_usd,
                ),
            },
        }),
        outcome => {
            let result = CheckResult::from_agent_outcome(
                check.name(),
                prepared.result_target(),
                outcome,
                config.model,
                config.input_per_1m_usd,
                config.output_per_1m_usd,
            );
            if let Some(cache) = config.cache {
                let _ = cache.write_json(&cache_key, &result);
            }

            Ok(CheckOutcome::success(result))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde::Serialize;
    use serde_json::json;

    use super::*;
    use crate::cache::CacheKey;
    use crate::git::CommitHash;
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
                conversation: vec![ConversationTurn::System("review the commit".to_string())],
                tools: Vec::new(),
                output_schema: json!({ "type": "object" }),
                target: PreparedCheckTarget::Commit(self.target.clone()),
            })
        }
    }

    #[derive(Debug, Serialize)]
    struct TestCheckCacheParams<'a> {
        commit: &'a CommitHash,
        review_context: &'a ReviewContext,
    }

    fn output(commit: &str) -> CheckOutput {
        CheckOutput {
            summary: "summary".to_string(),
            findings: vec![Finding {
                commit: CommitHash::new(commit).unwrap(),
                severity: Severity::Medium,
                message: "finding".to_string(),
                location: None,
            }],
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
            provider: "test",
            model: "test-model",
            max_iterations: 2,
            input_per_1m_usd: 2.0,
            output_per_1m_usd: 6.0,
            console: Console::default(),
            cache: None,
        }
    }

    fn config_with_cache<'a>(cache: &'a CacheStore) -> CheckRunConfig<'a> {
        CheckRunConfig {
            provider: "test",
            model: "test-model",
            max_iterations: 2,
            input_per_1m_usd: 2.0,
            output_per_1m_usd: 6.0,
            console: Console::default(),
            cache: Some(cache),
        }
    }

    fn extractor() -> Extractor {
        Extractor::new(PathBuf::from("/unused"), Console::default())
    }

    fn success_result(outcome: &CheckOutcome) -> &CheckResult {
        match outcome {
            CheckOutcome::Success { check } => check,
            CheckOutcome::NeedsUserInfo { .. } => panic!("expected successful check outcome"),
        }
    }

    fn request_user_info_call() -> crate::llm::provider::ToolCall {
        crate::llm::provider::ToolCall {
            id: "call-info".to_string(),
            name: "request_user_info".to_string(),
            arguments: json!({
                "questions": ["Which deployment policy applies, and why does it affect this check?"]
            }),
        }
    }

    #[tokio::test]
    async fn runs_prepared_check_and_builds_check_result() {
        let provider = MockProvider::new([Ok(response(output("abc1234")))]);
        let executor = FakeToolExecutor::default();
        let check = TestCheck {
            target: CommitHash::new("abc1234").unwrap(),
        };

        let outcome = run_check(
            &check,
            &extractor(),
            &provider,
            &executor,
            config(),
            &ReviewContext::default(),
        )
        .await
        .unwrap();
        let result = success_result(&outcome);

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
    async fn accepts_valid_result_without_retrying() {
        let provider = MockProvider::new([Ok(response(output("abc1234")))]);
        let executor = FakeToolExecutor::default();
        let check = TestCheck {
            target: CommitHash::new("abc1234").unwrap(),
        };

        let outcome = run_check(
            &check,
            &extractor(),
            &provider,
            &executor,
            config(),
            &ReviewContext::default(),
        )
        .await
        .unwrap();
        let result = success_result(&outcome);

        assert!(!result.is_exhausted);
        assert_eq!(result.exhaustion_reason, None);
        assert_eq!(result.iterations, 1);
        assert_eq!(provider.requests().len(), 1);
    }

    #[tokio::test]
    async fn retries_output_for_the_wrong_target() {
        let provider = MockProvider::new([
            Ok(response(output("def5678"))),
            Ok(response(output("abc1234"))),
        ]);
        let executor = FakeToolExecutor::default();
        let check = TestCheck {
            target: CommitHash::new("abc1234").unwrap(),
        };

        let outcome = run_check(
            &check,
            &extractor(),
            &provider,
            &executor,
            config(),
            &ReviewContext::default(),
        )
        .await
        .unwrap();
        let result = success_result(&outcome);

        assert_eq!(
            result.findings[0].commit,
            CommitHash::new("abc1234").unwrap()
        );
        assert_eq!(result.iterations, 2);
    }

    #[tokio::test]
    async fn returns_cached_check_result_without_calling_provider() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = CacheStore::new(tmp.path().join("cache"), Console::default());
        let executor = FakeToolExecutor::default();
        let check = TestCheck {
            target: CommitHash::new("abc1234").unwrap(),
        };
        let first_provider = MockProvider::new([Ok(response(output("abc1234")))]);

        let first_outcome = run_check(
            &check,
            &extractor(),
            &first_provider,
            &executor,
            config_with_cache(&cache),
            &ReviewContext::default(),
        )
        .await
        .unwrap();
        let first = success_result(&first_outcome);

        let second_provider = MockProvider::default();
        let second_outcome = run_check(
            &check,
            &extractor(),
            &second_provider,
            &executor,
            config_with_cache(&cache),
            &ReviewContext::default(),
        )
        .await
        .unwrap();
        let second = success_result(&second_outcome);

        assert_eq!(first_provider.requests().len(), 1);
        assert_eq!(second_provider.requests().len(), 0);
        assert_eq!(second.summary, first.summary);
        assert_eq!(second.findings, first.findings);
        assert_eq!(second.usage.input_tokens, 0);
        assert_eq!(second.usage.output_tokens, 0);
        assert_eq!(second.usage.cost_usd, 0.0);
    }

    #[tokio::test]
    async fn maps_user_info_request_to_check_outcome() {
        let provider = MockProvider::new([Ok(LlmCallResult {
            response: LlmResponse::ToolCalls(vec![request_user_info_call()]),
            usage: RawUsage {
                input_tokens: 1_000,
                output_tokens: 500,
            },
        })]);
        let executor = FakeToolExecutor::default();
        let check = TestCheck {
            target: CommitHash::new("abc1234").unwrap(),
        };

        let outcome = run_check(
            &check,
            &extractor(),
            &provider,
            &executor,
            config(),
            &ReviewContext::default(),
        )
        .await
        .unwrap();

        let CheckOutcome::NeedsUserInfo { request } = outcome else {
            panic!("expected user-info outcome");
        };
        assert_eq!(request.check, "test");
        assert_eq!(
            request.target,
            crate::llm::result::CheckTarget::Commit(CommitHash::new("abc1234").unwrap())
        );
        assert_eq!(
            request.questions,
            vec!["Which deployment policy applies, and why does it affect this check?"]
        );
        assert_eq!(request.iterations, 1);
        assert_eq!(request.usage.input_tokens, 1_000);
        assert_eq!(request.usage.output_tokens, 500);
        assert_eq!(request.usage.cost_usd, 0.005);
        assert!(executor.calls().is_empty());
    }

    #[tokio::test]
    async fn does_not_cache_user_info_request() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = CacheStore::new(tmp.path().join("cache"), Console::default());
        let executor = FakeToolExecutor::default();
        let check = TestCheck {
            target: CommitHash::new("abc1234").unwrap(),
        };
        let first_provider = MockProvider::new([Ok(LlmCallResult {
            response: LlmResponse::ToolCalls(vec![request_user_info_call()]),
            usage: RawUsage {
                input_tokens: 1,
                output_tokens: 1,
            },
        })]);

        run_check(
            &check,
            &extractor(),
            &first_provider,
            &executor,
            config_with_cache(&cache),
            &ReviewContext::default(),
        )
        .await
        .unwrap();

        let second_provider = MockProvider::new([Ok(response(output("abc1234")))]);
        let second_outcome = run_check(
            &check,
            &extractor(),
            &second_provider,
            &executor,
            config_with_cache(&cache),
            &ReviewContext::default(),
        )
        .await
        .unwrap();

        assert_eq!(first_provider.requests().len(), 1);
        assert_eq!(second_provider.requests().len(), 1);
        let second = success_result(&second_outcome);
        assert_eq!(second.summary, "summary");
    }
}
