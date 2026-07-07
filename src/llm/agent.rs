use std::fmt;

use crate::console::Console;
use crate::llm::confidence::Confidence;
use crate::llm::provider::{
    ConversationTurn, LlmCallError, LlmOutputMode, LlmProvider, LlmRequest, LlmResponse, RawUsage,
    ToolCall, ToolSpec,
};
use crate::llm::result::CheckOutput;

const LOW_CONFIDENCE_REFINEMENT_INSTRUCTION: &str = "Your previous check result was below the required confidence threshold. Continue the analysis, use additional tools if needed, and submit a revised check result.";

pub struct AgentRequest<'a> {
    pub model: &'a str,
    pub conversation: &'a [ConversationTurn],
    pub tools: &'a [ToolSpec],
    pub output_schema: &'a serde_json::Value,
    pub validate_output: &'a dyn Fn(&CheckOutput) -> Result<(), String>,
    pub confidence_threshold: Confidence,
    pub max_iterations: u32,
    pub console: Console,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentRunResult {
    pub output: CheckOutput,
    pub usage: RawUsage,
    pub iterations: u32,
}

#[derive(Debug)]
pub enum AgentRunOutcome {
    Completed(AgentRunResult),
    Exhausted {
        result: AgentRunResult,
        reason: AgentExhaustionReason,
    },
}

#[derive(Debug)]
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
    let mut best_low_confidence_output: Option<CheckOutput> = None;

    for iteration in 1..=request.max_iterations {
        let result = match provider
            .send(LlmRequest {
                model: request.model,
                conversation: &conversation,
                output_mode: LlmOutputMode::Check {
                    tools: request.tools,
                    output_schema: request.output_schema,
                },
            })
            .await
        {
            Ok(result) => result,
            Err(error) => {
                let Some(output) = best_low_confidence_output else {
                    return Err(error);
                };
                return Ok(AgentRunOutcome::Exhausted {
                    result: AgentRunResult {
                        output,
                        usage,
                        iterations: iteration,
                    },
                    reason: AgentExhaustionReason::LlmCall(error),
                });
            }
        };
        usage += result.usage;

        match result.response {
            LlmResponse::CheckOutput(output) => {
                if let Err(error) = (request.validate_output)(&output) {
                    request.console.debug(format_args!(
                        "llm iteration {iteration}: invalid check output: {error}"
                    ));
                    conversation.push(ConversationTurn::AssistantCheckOutput(output));
                    conversation.push(ConversationTurn::User(
                        invalid_output_refinement_instruction(&error),
                    ));
                    continue;
                }

                request.console.debug(format_args!(
                    "llm iteration {iteration}: check output confidence={} threshold={}",
                    output.confidence.as_f64(),
                    request.confidence_threshold.as_f64()
                ));
                if output.confidence >= request.confidence_threshold {
                    return Ok(AgentRunOutcome::Completed(AgentRunResult {
                        output,
                        usage,
                        iterations: iteration,
                    }));
                }

                conversation.push(ConversationTurn::AssistantCheckOutput(output.clone()));
                conversation.push(ConversationTurn::User(
                    LOW_CONFIDENCE_REFINEMENT_INSTRUCTION.to_string(),
                ));
                if best_low_confidence_output
                    .as_ref()
                    .is_none_or(|best| output.confidence >= best.confidence)
                {
                    best_low_confidence_output = Some(output);
                }
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
                let assistant_tool_calls = tool_calls.clone();
                conversation.push(ConversationTurn::AssistantToolCalls(assistant_tool_calls));

                // TODO: Execute independent tool calls concurrently.
                for tool_call in tool_calls {
                    let call_id = tool_call.id.clone();
                    let result = tool_executor.execute(tool_call).await;
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

    if let Some(output) = best_low_confidence_output {
        return Ok(AgentRunOutcome::Exhausted {
            result: AgentRunResult {
                output,
                usage,
                iterations: request.max_iterations,
            },
            reason: AgentExhaustionReason::MaxIterations,
        });
    }

    Err(LlmCallError::Permanent {
        message: format!(
            "LLM agent did not produce check output within {} iterations",
            request.max_iterations
        ),
        source: Box::new(AgentError::LoopExhausted),
    })
}

fn invalid_output_refinement_instruction<E>(error: &E) -> String
where
    E: fmt::Display,
{
    format!(
        "Your previous check result was invalid: {error}. Correct the result and submit it again."
    )
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

#[derive(Debug)]
enum AgentError {
    LoopExhausted,
    UnexpectedTextResponse,
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
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
    use serde_json::json;

    use super::*;
    use crate::llm::provider::{LlmCallResult, LlmResponse};
    use crate::llm::test_support::{FakeToolExecutor, MockProvider};

    fn check_output(summary: &str) -> CheckOutput {
        check_output_with_confidence(summary, 0.9)
    }

    fn check_output_with_confidence(summary: &str, confidence: f64) -> CheckOutput {
        CheckOutput {
            summary: summary.to_string(),
            findings: Vec::new(),
            confidence: Confidence::try_from(confidence).unwrap(),
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
            confidence_threshold: Confidence::try_from(0.8).unwrap(),
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

    #[tokio::test]
    async fn returns_check_output_without_tool_call() {
        let provider = MockProvider::new([Ok(call_result(
            LlmResponse::CheckOutput(check_output("done")),
            10,
            5,
        ))]);
        let executor = FakeToolExecutor::new([]);
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
    async fn retries_when_check_output_confidence_is_below_threshold() {
        let provider = MockProvider::new([
            Ok(call_result(
                LlmResponse::CheckOutput(check_output_with_confidence("uncertain", 0.7)),
                10,
                5,
            )),
            Ok(call_result(
                LlmResponse::CheckOutput(check_output_with_confidence("revised", 0.9)),
                20,
                7,
            )),
        ]);
        let executor = FakeToolExecutor::new([]);
        let schema = json!({ "type": "object" });

        let result = completed(
            run_agent(&provider, &executor, agent_request(&[], &[], &schema, 3))
                .await
                .unwrap(),
        );

        assert_eq!(result.output.summary, "revised");
        assert_eq!(result.iterations, 2);
        assert_eq!(result.usage.input_tokens, 30);
        assert_eq!(result.usage.output_tokens, 12);

        let requests = provider.requests();
        assert_eq!(requests.len(), 2);
        let ConversationTurn::AssistantCheckOutput(previous) = &requests[1].conversation[0] else {
            panic!("expected previous check output");
        };
        assert_eq!(previous.summary, "uncertain");
        let ConversationTurn::User(instruction) = &requests[1].conversation[1] else {
            panic!("expected refinement instruction");
        };
        assert_eq!(instruction, LOW_CONFIDENCE_REFINEMENT_INSTRUCTION);
    }

    #[tokio::test]
    async fn accepts_confidence_equal_to_threshold() {
        let provider = MockProvider::new([Ok(call_result(
            LlmResponse::CheckOutput(check_output_with_confidence("enough", 0.8)),
            10,
            5,
        ))]);
        let executor = FakeToolExecutor::new([]);
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
    async fn retries_when_check_output_validation_fails() {
        let provider = MockProvider::new([
            Ok(call_result(
                LlmResponse::CheckOutput(check_output("invalid")),
                10,
                5,
            )),
            Ok(call_result(
                LlmResponse::CheckOutput(check_output("valid")),
                20,
                7,
            )),
        ]);
        let executor = FakeToolExecutor::new([]);
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

        let result = completed(run_agent(&provider, &executor, request).await.unwrap());

        assert_eq!(result.output.summary, "valid");
        assert_eq!(result.iterations, 2);
        let requests = provider.requests();
        assert_eq!(requests.len(), 2);
        let ConversationTurn::User(instruction) = &requests[1].conversation[1] else {
            panic!("expected validation refinement instruction");
        };
        assert!(instruction.contains("summary must be valid"));
    }

    #[tokio::test]
    async fn returns_highest_confidence_output_when_confidence_remains_below_threshold() {
        let provider = MockProvider::new([
            Ok(call_result(
                LlmResponse::CheckOutput(check_output_with_confidence("first", 0.7)),
                10,
                5,
            )),
            Ok(call_result(
                LlmResponse::CheckOutput(check_output_with_confidence("second", 0.6)),
                20,
                7,
            )),
        ]);
        let executor = FakeToolExecutor::new([]);
        let schema = json!({ "type": "object" });

        let outcome = run_agent(&provider, &executor, agent_request(&[], &[], &schema, 2))
            .await
            .unwrap();

        let AgentRunOutcome::Exhausted { result, reason } = outcome else {
            panic!("expected exhausted agent run");
        };
        assert_eq!(result.output.summary, "first");
        assert_eq!(result.output.confidence.as_f64(), 0.7);
        assert_eq!(result.iterations, 2);
        assert_eq!(result.usage.input_tokens, 30);
        assert_eq!(result.usage.output_tokens, 12);
        assert!(matches!(reason, AgentExhaustionReason::MaxIterations));
        assert_eq!(provider.requests().len(), 2);
    }

    #[tokio::test]
    async fn returns_last_low_confidence_output_when_later_llm_call_fails() {
        let provider = MockProvider::new([
            Ok(call_result(
                LlmResponse::CheckOutput(check_output_with_confidence("uncertain", 0.7)),
                10,
                5,
            )),
            Err(LlmCallError::Transient {
                message: "request timed out".to_string(),
                source: Box::new(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "request timed out",
                )),
            }),
        ]);
        let executor = FakeToolExecutor::new([]);
        let schema = json!({ "type": "object" });

        let outcome = run_agent(&provider, &executor, agent_request(&[], &[], &schema, 3))
            .await
            .unwrap();

        let AgentRunOutcome::Exhausted { result, reason } = outcome else {
            panic!("expected exhausted agent run");
        };
        assert_eq!(result.output.summary, "uncertain");
        assert_eq!(result.iterations, 2);
        assert_eq!(result.usage.input_tokens, 10);
        assert_eq!(result.usage.output_tokens, 5);
        assert!(matches!(
            reason,
            AgentExhaustionReason::LlmCall(LlmCallError::Transient { .. })
        ));
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
        let executor = FakeToolExecutor::new([]);
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
    async fn tool_failures_are_returned_to_model_as_tool_results() {
        let tool_call = ToolCall {
            id: "call-1".to_string(),
            name: "commit_diff".to_string(),
            arguments: json!({ "hash": "abc1234" }),
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
}
