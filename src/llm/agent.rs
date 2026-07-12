use std::fmt;

use futures_util::stream::{self, StreamExt};

use crate::console::Console;
use crate::llm::provider::{
    ConversationTurn, LlmCallError, LlmOutputMode, LlmProvider, LlmRequest, LlmResponse, RawUsage,
    ToolCall, ToolSpec,
};
use crate::llm::result::CheckOutput;

const REQUEST_USER_INFO_TOOL_NAME: &str = "request_user_info";

pub struct AgentRequest<'a> {
    pub model: &'a str,
    pub conversation: &'a [ConversationTurn],
    pub tools: &'a [ToolSpec],
    pub output_schema: &'a serde_json::Value,
    pub validate_output: &'a dyn Fn(&CheckOutput) -> Result<(), String>,
    pub max_iterations: u32,
    pub console: Console,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentRunResult {
    pub output: CheckOutput,
    pub usage: RawUsage,
    pub iterations: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentUserInfoRequest {
    pub questions: Vec<String>,
}

#[derive(Debug)]
#[allow(dead_code)]
pub enum AgentRunOutcome {
    Completed(AgentRunResult),
    Exhausted {
        result: AgentRunResult,
        reason: AgentExhaustionReason,
    },
    NeedsUserInfo {
        request: AgentUserInfoRequest,
        usage: RawUsage,
        iterations: u32,
    },
}

#[derive(Debug)]
#[allow(dead_code)]
pub enum AgentExhaustionReason {
    MaxIterations,
    LlmCall(LlmCallError),
}

pub type ToolExecutionResult = Result<serde_json::Value, Box<dyn std::error::Error>>;

pub trait ToolExecutor {
    async fn execute(&self, call: ToolCall) -> ToolExecutionResult;
}

pub async fn run_agent<P, E>(
    provider: &P,
    tool_executor: &E,
    request: AgentRequest<'_>,
) -> Result<AgentRunOutcome, LlmCallError>
where
    P: LlmProvider,
    E: ToolExecutor,
{
    let mut conversation = request.conversation.to_vec();
    let mut usage = RawUsage::default();
    for iteration in 1..=request.max_iterations {
        // Reserve the final turn for either emitting the structured check output or asking the
        // user for indispensable information. This prevents a late repository lookup from
        // consuming the turn needed to submit the result.
        let tools = if iteration == request.max_iterations {
            request
                .tools
                .iter()
                .filter(|tool| tool.name == REQUEST_USER_INFO_TOOL_NAME)
                .cloned()
                .collect::<Vec<_>>()
        } else {
            request.tools.to_vec()
        };
        let result = provider
            .send(LlmRequest {
                model: request.model,
                conversation: &conversation,
                output_mode: LlmOutputMode::Check {
                    tools: &tools,
                    output_schema: request.output_schema,
                },
            })
            .await?;
        usage += result.usage;

        match result.response {
            LlmResponse::CheckOutput(output) => {
                if let Err(error) = (request.validate_output)(&output) {
                    request.console.debug(format_args!(
                        "llm iteration {iteration}: invalid check output: {error}"
                    ));
                    return Err(LlmCallError::Permanent {
                        message: format!("invalid check output: {error}"),
                        source: Box::new(AgentError::InvalidCheckOutput),
                    });
                }

                return Ok(AgentRunOutcome::Completed(AgentRunResult {
                    output,
                    usage,
                    iterations: iteration,
                }));
            }
            LlmResponse::ToolCalls(tool_calls) => {
                request.console.debug(format_args!(
                    "llm iteration {iteration}: {} tool {}",
                    tool_calls.len(),
                    if tool_calls.len() <= 1 {
                        "call"
                    } else {
                        "calls"
                    }
                ));
                if let Some(tool_call) = tool_calls
                    .iter()
                    .find(|tool_call| tool_call.name == REQUEST_USER_INFO_TOOL_NAME)
                {
                    let user_info_request = parse_user_info_request(tool_call)?;
                    return Ok(AgentRunOutcome::NeedsUserInfo {
                        request: user_info_request,
                        usage,
                        iterations: iteration,
                    });
                }

                let assistant_tool_calls = tool_calls.clone();
                conversation.push(ConversationTurn::AssistantToolCalls(assistant_tool_calls));

                let tool_call_concurrency = std::thread::available_parallelism()
                    .map_or(2, |parallelism| parallelism.get().saturating_mul(2));
                let tool_results = stream::iter(tool_calls)
                    .map(|tool_call| async move {
                        let call_id = tool_call.id.clone();
                        let result = tool_executor.execute(tool_call).await;

                        (call_id, result)
                    })
                    .buffered(tool_call_concurrency)
                    .collect::<Vec<_>>()
                    .await;

                for (call_id, result) in tool_results {
                    conversation.push(ConversationTurn::ToolResult {
                        call_id,
                        result: tool_result_json(result),
                    });
                }
            }
            LlmResponse::Text(_) => {
                return Err(LlmCallError::Permanent {
                    message: "LLM returned text response while check output was expected"
                        .to_string(),
                    source: Box::new(AgentError::UnexpectedTextResponse),
                });
            }
        }
    }

    Err(LlmCallError::Permanent {
        message: format!(
            "LLM agent did not produce check output within {} iterations",
            request.max_iterations
        ),
        source: Box::new(AgentError::LoopExhausted),
    })
}

fn tool_result_json(result: ToolExecutionResult) -> serde_json::Value {
    match result {
        Ok(value) => value,
        Err(error) => {
            serde_json::json!({
                "error": error.to_string(),
            })
        }
    }
}

fn parse_user_info_request(tool_call: &ToolCall) -> Result<AgentUserInfoRequest, LlmCallError> {
    #[derive(serde::Deserialize)]
    struct RequestUserInfoArguments {
        questions: Vec<String>,
    }

    let arguments: RequestUserInfoArguments = serde_json::from_value(tool_call.arguments.clone())
        .map_err(|error| LlmCallError::Permanent {
        message: format!("invalid request_user_info arguments: {error}"),
        source: Box::new(AgentError::InvalidUserInfoRequest),
    })?;

    if arguments.questions.is_empty() {
        return Err(LlmCallError::Permanent {
            message: "invalid request_user_info arguments: questions must not be empty".to_string(),
            source: Box::new(AgentError::InvalidUserInfoRequest),
        });
    }

    Ok(AgentUserInfoRequest {
        questions: arguments.questions,
    })
}

#[derive(Debug)]
enum AgentError {
    InvalidCheckOutput,
    InvalidUserInfoRequest,
    LoopExhausted,
    UnexpectedTextResponse,
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCheckOutput => f.write_str("invalid check output"),
            Self::InvalidUserInfoRequest => f.write_str("invalid user info request"),
            Self::LoopExhausted => f.write_str("agent loop exhausted"),
            Self::UnexpectedTextResponse => f.write_str("unexpected text response"),
        }
    }
}

impl std::error::Error for AgentError {}

impl fmt::Display for AgentExhaustionReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MaxIterations => write!(f, "maximum iterations reached"),
            Self::LlmCall(error) => error.fmt(f),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use serde_json::json;

    use super::*;
    use crate::llm::provider::{LlmCallResult, LlmResponse};
    use crate::llm::test_support::{FakeToolExecutor, MockProvider, RecordedLlmOutputMode};

    fn check_output(summary: &str) -> CheckOutput {
        CheckOutput {
            summary: summary.to_string(),
            findings: Vec::new(),
        }
    }

    fn call_result(response: LlmResponse, input_tokens: u32, output_tokens: u32) -> LlmCallResult {
        LlmCallResult {
            response,
            usage: RawUsage {
                input_tokens,
                output_tokens,
            },
        }
    }

    fn agent_request<'a>(
        conversation: &'a [ConversationTurn],
        tools: &'a [ToolSpec],
        output_schema: &'a serde_json::Value,
        max_iterations: u32,
    ) -> AgentRequest<'a> {
        fn accept_output(_output: &CheckOutput) -> Result<(), String> {
            Ok(())
        }

        AgentRequest {
            model: "test-model",
            conversation,
            tools,
            output_schema,
            validate_output: &accept_output,
            max_iterations,
            console: Console::default(),
        }
    }

    fn completed(outcome: AgentRunOutcome) -> AgentRunResult {
        let AgentRunOutcome::Completed(result) = outcome else {
            panic!("expected completed agent run");
        };
        result
    }

    fn user_info_request(outcome: AgentRunOutcome) -> (AgentUserInfoRequest, RawUsage, u32) {
        let AgentRunOutcome::NeedsUserInfo {
            request,
            usage,
            iterations,
        } = outcome
        else {
            panic!("expected user-info request");
        };
        (request, usage, iterations)
    }

    struct ConcurrentToolExecutor {
        active_calls: AtomicUsize,
        max_active_calls: AtomicUsize,
    }

    impl ConcurrentToolExecutor {
        fn new() -> Self {
            Self {
                active_calls: AtomicUsize::new(0),
                max_active_calls: AtomicUsize::new(0),
            }
        }
    }

    impl ToolExecutor for ConcurrentToolExecutor {
        async fn execute(&self, call: ToolCall) -> ToolExecutionResult {
            let active_calls = self.active_calls.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active_calls
                .fetch_max(active_calls, Ordering::SeqCst);

            let delay = if call.id == "call-first" { 20 } else { 1 };
            tokio::time::sleep(Duration::from_millis(delay)).await;
            self.active_calls.fetch_sub(1, Ordering::SeqCst);

            Ok(json!({ "call_id": call.id }))
        }
    }

    #[tokio::test]
    async fn returns_check_output_without_tool_call() {
        let provider = MockProvider::new([Ok(call_result(
            LlmResponse::CheckOutput(check_output("done")),
            10,
            5,
        ))]);
        let executor = FakeToolExecutor::default();
        let schema = json!({ "type": "object" });
        let result = completed(
            run_agent(&provider, &executor, agent_request(&[], &[], &schema, 3))
                .await
                .unwrap(),
        );

        assert_eq!(result.output.summary, "done");
        assert_eq!(result.usage.input_tokens, 10);
        assert_eq!(result.usage.output_tokens, 5);
        assert_eq!(result.iterations, 1);
    }

    #[tokio::test]
    async fn accepts_check_output() {
        let provider = MockProvider::new([Ok(call_result(
            LlmResponse::CheckOutput(check_output("uncertain")),
            10,
            5,
        ))]);
        let executor = FakeToolExecutor::default();
        let schema = json!({ "type": "object" });

        let result = completed(
            run_agent(&provider, &executor, agent_request(&[], &[], &schema, 3))
                .await
                .unwrap(),
        );

        assert_eq!(result.output.summary, "uncertain");
        assert_eq!(result.iterations, 1);
        assert_eq!(result.usage.input_tokens, 10);
        assert_eq!(result.usage.output_tokens, 5);
        assert_eq!(provider.requests().len(), 1);
    }

    #[tokio::test]
    async fn accepts_valid_check_output() {
        let provider = MockProvider::new([Ok(call_result(
            LlmResponse::CheckOutput(check_output("enough")),
            10,
            5,
        ))]);
        let executor = FakeToolExecutor::default();
        let schema = json!({ "type": "object" });

        let result = completed(
            run_agent(&provider, &executor, agent_request(&[], &[], &schema, 3))
                .await
                .unwrap(),
        );

        assert_eq!(result.output.summary, "enough");
        assert_eq!(result.iterations, 1);
        assert_eq!(provider.requests().len(), 1);
    }

    #[tokio::test]
    async fn returns_error_when_check_output_validation_fails() {
        let provider = MockProvider::new([Ok(call_result(
            LlmResponse::CheckOutput(check_output("invalid")),
            10,
            5,
        ))]);
        let executor = FakeToolExecutor::default();
        let schema = json!({ "type": "object" });
        let validate = |output: &CheckOutput| {
            if output.summary == "valid" {
                Ok(())
            } else {
                Err("summary must be valid".to_string())
            }
        };
        let mut request = agent_request(&[], &[], &schema, 2);
        request.validate_output = &validate;

        let error = run_agent(&provider, &executor, request).await.unwrap_err();

        assert!(matches!(error, LlmCallError::Permanent { .. }));
        assert_eq!(
            error.to_string(),
            "permanent LLM call failure: invalid check output: summary must be valid"
        );
        assert_eq!(provider.requests().len(), 1);
    }

    #[tokio::test]
    async fn propagates_llm_call_error_without_prior_check_output() {
        let provider = MockProvider::new([Err(LlmCallError::Transient {
            message: "request timed out".to_string(),
            source: Box::new(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "request timed out",
            )),
        })]);
        let executor = FakeToolExecutor::default();
        let schema = json!({ "type": "object" });

        let error = run_agent(&provider, &executor, agent_request(&[], &[], &schema, 3))
            .await
            .unwrap_err();

        assert!(matches!(error, LlmCallError::Transient { .. }));
    }

    #[tokio::test]
    async fn executes_tool_calls_and_sends_results_to_next_iteration() {
        let tool_call = ToolCall {
            id: "call-1".to_string(),
            name: "commit_diff".to_string(),
            arguments: json!({ "hash": "abc1234" }),
            thought_signature: None,
        };
        let provider = MockProvider::new([
            Ok(call_result(
                LlmResponse::ToolCalls(vec![tool_call.clone()]),
                10,
                5,
            )),
            Ok(call_result(
                LlmResponse::CheckOutput(check_output("done")),
                20,
                7,
            )),
        ]);
        let executor = FakeToolExecutor::new([Ok(json!({
            "diff": "+hello"
        }))]);
        let conversation = [ConversationTurn::User("check abc1234".to_string())];
        let tools = [ToolSpec {
            name: "commit_diff".to_string(),
            description: "Read a commit diff".to_string(),
            parameters: json!({ "type": "object" }),
        }];
        let schema = json!({ "type": "object" });

        let result = completed(
            run_agent(
                &provider,
                &executor,
                agent_request(&conversation, &tools, &schema, 3),
            )
            .await
            .unwrap(),
        );

        assert_eq!(result.output.summary, "done");
        assert_eq!(result.usage.input_tokens, 30);
        assert_eq!(result.usage.output_tokens, 12);
        assert_eq!(result.iterations, 2);
        assert_eq!(executor.calls(), vec![tool_call.clone()]);

        let requests = provider.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1].conversation.len(), 3);
        assert!(matches!(
            requests[1].conversation[1],
            ConversationTurn::AssistantToolCalls(_)
        ));
        let ConversationTurn::ToolResult { call_id, result } = &requests[1].conversation[2] else {
            panic!("expected tool result");
        };
        assert_eq!(call_id, "call-1");
        assert_eq!(result, &json!({ "diff": "+hello" }));
    }

    #[tokio::test]
    async fn final_iteration_exposes_only_user_info_tool() {
        let tool_call = ToolCall {
            id: "call-1".to_string(),
            name: "commit_diff".to_string(),
            arguments: json!({ "hash": "abc1234" }),
            thought_signature: None,
        };
        let provider = MockProvider::new([
            Ok(call_result(LlmResponse::ToolCalls(vec![tool_call]), 10, 5)),
            Ok(call_result(
                LlmResponse::CheckOutput(check_output("done")),
                20,
                7,
            )),
        ]);
        let executor = FakeToolExecutor::new([Ok(json!({ "diff": "+hello" }))]);
        let tools = [
            ToolSpec {
                name: "commit_diff".to_string(),
                description: "Read a commit diff".to_string(),
                parameters: json!({ "type": "object" }),
            },
            ToolSpec {
                name: REQUEST_USER_INFO_TOOL_NAME.to_string(),
                description: "Ask the user for necessary context".to_string(),
                parameters: json!({ "type": "object" }),
            },
        ];
        let schema = json!({ "type": "object" });

        run_agent(&provider, &executor, agent_request(&[], &tools, &schema, 2))
            .await
            .unwrap();

        let requests = provider.requests();
        let RecordedLlmOutputMode::Check { tools, .. } = &requests[1].output_mode else {
            panic!("expected check output mode");
        };
        assert_eq!(
            tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            [REQUEST_USER_INFO_TOOL_NAME]
        );
    }

    #[tokio::test]
    async fn executes_independent_tool_calls_concurrently_in_request_order() {
        let first_call = ToolCall {
            id: "call-first".to_string(),
            name: "get_commit_diff".to_string(),
            arguments: json!({ "revision": "abc1234" }),
            thought_signature: None,
        };
        let additional_calls = ["second", "third", "fourth", "fifth"].map(|suffix| ToolCall {
            id: format!("call-{suffix}"),
            name: "get_changed_files".to_string(),
            arguments: json!({ "revision": "abc1234" }),
            thought_signature: None,
        });
        let provider = MockProvider::new([
            Ok(call_result(
                LlmResponse::ToolCalls(
                    std::iter::once(first_call)
                        .chain(additional_calls)
                        .collect(),
                ),
                10,
                5,
            )),
            Ok(call_result(
                LlmResponse::CheckOutput(check_output("done")),
                20,
                7,
            )),
        ]);
        let executor = ConcurrentToolExecutor::new();
        let schema = json!({ "type": "object" });

        run_agent(&provider, &executor, agent_request(&[], &[], &schema, 3))
            .await
            .unwrap();

        assert!(executor.max_active_calls.load(Ordering::SeqCst) >= 2);

        let requests = provider.requests();
        let tool_results = requests[1]
            .conversation
            .iter()
            .filter_map(|turn| match turn {
                ConversationTurn::ToolResult { call_id, result } => Some((call_id, result)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(tool_results.len(), 5);
        for ((call_id, result), suffix) in tool_results
            .into_iter()
            .zip(["first", "second", "third", "fourth", "fifth"])
        {
            let expected_call_id = format!("call-{suffix}");
            assert_eq!(call_id, &expected_call_id);
            assert_eq!(result, &json!({ "call_id": expected_call_id }));
        }
    }

    #[tokio::test]
    async fn tool_failures_are_returned_to_model_as_tool_results() {
        let tool_call = ToolCall {
            id: "call-1".to_string(),
            name: "commit_diff".to_string(),
            arguments: json!({ "hash": "abc1234" }),
            thought_signature: None,
        };
        let provider = MockProvider::new([
            Ok(call_result(LlmResponse::ToolCalls(vec![tool_call]), 10, 5)),
            Ok(call_result(
                LlmResponse::CheckOutput(check_output("done")),
                20,
                7,
            )),
        ]);
        let executor = FakeToolExecutor::new([Err(
            Box::new(std::io::Error::other("git failed")) as Box<dyn std::error::Error>
        )]);
        let schema = json!({ "type": "object" });

        run_agent(&provider, &executor, agent_request(&[], &[], &schema, 3))
            .await
            .unwrap();

        let requests = provider.requests();
        let ConversationTurn::ToolResult { result, .. } = &requests[1].conversation[1] else {
            panic!("expected tool result");
        };
        assert_eq!(result, &json!({ "error": "git failed" }));
    }

    #[tokio::test]
    async fn returns_user_info_request_when_tool_is_called() {
        let tool_call = ToolCall {
            id: "call-1".to_string(),
            name: "request_user_info".to_string(),
            arguments: json!({
                "questions": [
                    "What production auth policy applies here, and why it affects this security check?"
                ]
            }),
            thought_signature: None,
        };
        let provider = MockProvider::new([Ok(call_result(
            LlmResponse::ToolCalls(vec![tool_call]),
            10,
            5,
        ))]);
        let executor = FakeToolExecutor::default();
        let schema = json!({ "type": "object" });

        let (request, usage, iterations) = user_info_request(
            run_agent(&provider, &executor, agent_request(&[], &[], &schema, 3))
                .await
                .unwrap(),
        );

        assert_eq!(
            request.questions,
            vec![
                "What production auth policy applies here, and why it affects this security check?"
            ]
        );
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 5);
        assert_eq!(iterations, 1);
        assert!(executor.calls().is_empty());
    }

    #[tokio::test]
    async fn request_user_info_ignores_other_tool_calls() {
        let info_call = ToolCall {
            id: "call-info".to_string(),
            name: "request_user_info".to_string(),
            arguments: json!({
                "questions": ["Which deployment flag is enabled, and why does it affect this check?"]
            }),
            thought_signature: None,
        };
        let diff_call = ToolCall {
            id: "call-diff".to_string(),
            name: "get_commit_diff".to_string(),
            arguments: json!({ "revision": "abc1234" }),
            thought_signature: None,
        };
        let provider = MockProvider::new([Ok(call_result(
            LlmResponse::ToolCalls(vec![diff_call, info_call]),
            10,
            5,
        ))]);
        let executor = FakeToolExecutor::default();
        let schema = json!({ "type": "object" });

        let (request, _, _) = user_info_request(
            run_agent(&provider, &executor, agent_request(&[], &[], &schema, 3))
                .await
                .unwrap(),
        );

        assert_eq!(
            request.questions,
            vec!["Which deployment flag is enabled, and why does it affect this check?"]
        );
        assert!(executor.calls().is_empty());
    }
}
