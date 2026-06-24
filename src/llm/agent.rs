use std::fmt;

use crate::console::Console;
use crate::llm::confidence::Confidence;
use crate::llm::provider::{
    ConversationTurn, LlmCallError, LlmProvider, LlmRequest, LlmResponse, RawUsage, ToolCall,
    ToolSpec,
};
use crate::llm::result::CheckOutput;

const LOW_CONFIDENCE_REFINEMENT_INSTRUCTION: &str = "Your previous check result was below the required confidence threshold. Continue the analysis, use additional tools if needed, and submit a revised check result.";

pub struct AgentRequest<'a> {
    pub model: &'a str,
    pub conversation: &'a [ConversationTurn],
    pub tools: &'a [ToolSpec],
    pub output_schema: &'a serde_json::Value,
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

pub type ToolExecutionResult = Result<serde_json::Value, Box<dyn std::error::Error>>;

pub trait ToolExecutor {
    async fn execute(&self, call: ToolCall) -> ToolExecutionResult;
}

#[allow(dead_code)]
pub async fn run_agent<P, E>(
    provider: &P,
    tool_executor: &E,
    request: AgentRequest<'_>,
) -> Result<AgentRunResult, LlmCallError>
where
    P: LlmProvider,
    E: ToolExecutor,
{
    let mut conversation = request.conversation.to_vec();
    let mut usage = RawUsage::default();

    for iteration in 1..=request.max_iterations {
        let result = provider
            .send(LlmRequest {
                model: request.model,
                conversation: &conversation,
                tools: request.tools,
                output_schema: request.output_schema,
            })
            .await?;
        usage += result.usage;

        match result.response {
            LlmResponse::CheckOutput(output) => {
                request.console.debug(format!(
                    "llm iteration {iteration}: check output confidence={} threshold={}",
                    output.confidence.as_f64(),
                    request.confidence_threshold.as_f64()
                ));
                if output.confidence >= request.confidence_threshold {
                    return Ok(AgentRunResult {
                        output,
                        usage,
                        iterations: iteration,
                    });
                }

                conversation.push(ConversationTurn::AssistantCheckOutput(output));
                conversation.push(ConversationTurn::User(
                    LOW_CONFIDENCE_REFINEMENT_INSTRUCTION.to_string(),
                ));
            }
            LlmResponse::ToolCalls(tool_calls) => {
                request.console.debug(format!(
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
        }
    }

    Err(LlmCallError::Permanent {
        message: format!(
            "LLM agent did not produce check output within {} iterations",
            request.max_iterations
        ),
        source: Box::new(AgentLoopError),
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

#[derive(Debug)]
struct AgentLoopError;

impl fmt::Display for AgentLoopError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "agent loop exhausted")
    }
}

impl std::error::Error for AgentLoopError {}

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
        AgentRequest {
            model: "test-model",
            conversation,
            tools,
            output_schema,
            confidence_threshold: Confidence::try_from(0.8).unwrap(),
            max_iterations,
            console: Console::default(),
        }
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
        let result = run_agent(&provider, &executor, agent_request(&[], &[], &schema, 3))
            .await
            .unwrap();

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

        let result = run_agent(&provider, &executor, agent_request(&[], &[], &schema, 3))
            .await
            .unwrap();

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

        let result = run_agent(&provider, &executor, agent_request(&[], &[], &schema, 3))
            .await
            .unwrap();

        assert_eq!(result.output.summary, "enough");
        assert_eq!(result.iterations, 1);
        assert_eq!(provider.requests().len(), 1);
    }

    #[tokio::test]
    async fn fails_when_confidence_remains_below_threshold() {
        let provider = MockProvider::new([
            Ok(call_result(
                LlmResponse::CheckOutput(check_output_with_confidence("first", 0.6)),
                10,
                5,
            )),
            Ok(call_result(
                LlmResponse::CheckOutput(check_output_with_confidence("second", 0.7)),
                20,
                7,
            )),
        ]);
        let executor = FakeToolExecutor::new([]);
        let schema = json!({ "type": "object" });

        let error = run_agent(&provider, &executor, agent_request(&[], &[], &schema, 2))
            .await
            .unwrap_err();

        assert!(matches!(error, LlmCallError::Permanent { .. }));
        assert_eq!(provider.requests().len(), 2);
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

        let result = run_agent(
            &provider,
            &executor,
            agent_request(&conversation, &tools, &schema, 3),
        )
        .await
        .unwrap();

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
