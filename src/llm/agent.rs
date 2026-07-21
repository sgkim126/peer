#![cfg_attr(not(test), expect(dead_code))]

use std::fmt;

use crate::console::Console;
use crate::llm::provider::{
    ConversationTurn, LlmCallError, LlmProvider, LlmRequest, LlmResponse, LlmTransport, RawUsage,
    ToolCall, ToolSpec,
};
use crate::llm::result::CheckOutput;
use crate::llm::tools::{
    ToolExecutionResult, ToolExecutor, request_clarification, submit_check_result,
};

use serde_json::json;

pub struct AgentRequest {
    pub model: String,
    pub conversation: Vec<ConversationTurn>,
    pub tools: Vec<ToolSpec>,
}

pub struct Agent<P, T, E>
where
    P: LlmProvider,
    T: LlmTransport,
    E: ToolExecutor,
{
    provider: P,
    transport: T,
    tool_executor: E,
    console: Console,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentCompletion {
    pub output: CheckOutput,
    pub usage: RawUsage,
    pub iterations: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentClarification {
    pub questions: Vec<String>,
    pub usage: RawUsage,
    pub iterations: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AgentOutcome {
    Completed(AgentCompletion),
    ClarificationRequested(AgentClarification),
}

impl<P, T, E> Agent<P, T, E>
where
    P: LlmProvider,
    T: LlmTransport,
    E: ToolExecutor,
{
    pub fn new(provider: P, transport: T, tool_executor: E, console: Console) -> Self {
        Self {
            provider,
            transport,
            tool_executor,
            console,
        }
    }

    pub async fn run_loop(
        &self,
        request: AgentRequest,
        max_iterations: u32,
    ) -> Result<AgentOutcome, LlmCallError> {
        let clarification_tool_name = request_clarification().name;
        let submit_result_tool_name = submit_check_result().name;
        let terminal_tools = terminal_tools(
            &request.tools,
            [&submit_result_tool_name, &clarification_tool_name],
        );
        let mut conversation = request.conversation;
        let mut usage = RawUsage::default();

        for iteration in 1..=max_iterations {
            let is_last_request = iteration == max_iterations;
            let tools = if is_last_request {
                &terminal_tools
            } else {
                &request.tools
            };
            let http_request = self.provider.build_request(
                LlmRequest {
                    model: &request.model,
                    conversation: &conversation,
                    tools,
                },
                is_last_request,
            )?;
            let response = self.transport.send(http_request).await?;
            let result = self.provider.parse_response(response)?;
            usage += result.usage;

            let LlmResponse::ToolCalls(tool_calls) = result.response;
            self.console.debug(format_args!(
                "agent iteration {iteration}/{}: {}",
                max_iterations,
                tool_calls
                    .iter()
                    .map(|tool_call| tool_call.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            ));

            // TODO: Define a policy for responses containing both
            // `request_clarification` and `submit_check_result` tool calls.
            if let Some(tool_call) = tool_calls
                .iter()
                .find(|tool_call| tool_call.name == clarification_tool_name)
            {
                return Ok(AgentOutcome::ClarificationRequested(AgentClarification {
                    questions: parse_clarification_questions(tool_call)?,
                    usage,
                    iterations: iteration,
                }));
            }

            if let Some(tool_call) = tool_calls
                .iter()
                .find(|tool_call| tool_call.name == submit_result_tool_name)
            {
                let output =
                    serde_json::from_value(tool_call.arguments.clone()).map_err(|error| {
                        permanent_error(
                            format!("invalid submit_check_result arguments: {error}"),
                            AgentError::InvalidCheckOutput { source: error },
                        )
                    })?;

                return Ok(AgentOutcome::Completed(AgentCompletion {
                    output,
                    usage,
                    iterations: iteration,
                }));
            }

            if is_last_request {
                return Err(permanent_error(
                    format!(
                        "LLM agent did not submit a check result within {} iterations",
                        max_iterations
                    ),
                    AgentError::LoopExhausted,
                ));
            }

            conversation.push(ConversationTurn::AssistantToolCalls(tool_calls.clone()));
            for tool_call in tool_calls {
                let call_id = tool_call.id.clone();
                let result = self.tool_executor.execute(tool_call).await;
                conversation.push(ConversationTurn::ToolResult {
                    call_id,
                    result: tool_result_json(result),
                });
            }
        }
        Err(permanent_error(
            format!(
                "LLM agent did not submit a check result within {} iterations",
                max_iterations
            ),
            AgentError::LoopExhausted,
        ))
    }
}

fn terminal_tools(tools: &[ToolSpec], terminal_tool_names: [&str; 2]) -> Vec<ToolSpec> {
    tools
        .iter()
        .filter(|tool| terminal_tool_names.contains(&tool.name.as_str()))
        .cloned()
        .collect()
}

fn parse_clarification_questions(tool_call: &ToolCall) -> Result<Vec<String>, LlmCallError> {
    #[derive(serde::Deserialize)]
    struct ClarificationArguments {
        questions: Vec<String>,
    }

    let arguments: ClarificationArguments = serde_json::from_value(tool_call.arguments.clone())
        .map_err(|error| {
            permanent_error(
                format!("invalid request_clarification arguments: {error}"),
                AgentError::InvalidClarificationRequest {
                    source: Some(Box::new(error)),
                },
            )
        })?;
    if arguments.questions.is_empty() {
        return Err(permanent_error(
            "invalid request_clarification arguments: questions must not be empty".to_string(),
            AgentError::InvalidClarificationRequest { source: None },
        ));
    }

    Ok(arguments.questions)
}

fn tool_result_json(result: ToolExecutionResult) -> serde_json::Value {
    match result {
        Ok(value) => value,
        Err(error) => json!({ "error": error.to_string() }),
    }
}

fn permanent_error(message: String, source: AgentError) -> LlmCallError {
    LlmCallError::Permanent {
        message,
        source: Box::new(source),
    }
}

#[derive(Debug)]
enum AgentError {
    InvalidCheckOutput {
        source: serde_json::Error,
    },
    InvalidClarificationRequest {
        source: Option<Box<dyn std::error::Error>>,
    },
    LoopExhausted,
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCheckOutput { .. } => f.write_str("invalid check output"),
            Self::InvalidClarificationRequest { .. } => {
                f.write_str("invalid clarification request")
            }
            Self::LoopExhausted => f.write_str("agent loop exhausted"),
        }
    }
}

impl std::error::Error for AgentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidCheckOutput { source } => Some(source),
            Self::InvalidClarificationRequest { source } => source.as_deref(),
            Self::LoopExhausted => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::VecDeque;
    use std::sync::Mutex;

    use reqwest::StatusCode;

    use crate::llm::provider::{LlmCallResult, Request, Response};
    use crate::llm::test_support::MockProvider;
    use crate::llm::tools::{ToolExecutionError, request_clarification, submit_check_result};

    struct TestTransport {
        responses: Mutex<VecDeque<Result<Response, LlmCallError>>>,
    }

    impl TestTransport {
        fn new(responses: impl IntoIterator<Item = Result<Response, LlmCallError>>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().collect()),
            }
        }
    }

    impl LlmTransport for TestTransport {
        async fn send(&self, _request: Request) -> Result<Response, LlmCallError> {
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| panic!("TestTransport has no queued response"))
        }
    }

    struct EchoToolExecutor;

    impl ToolExecutor for EchoToolExecutor {
        async fn execute(&self, call: ToolCall) -> ToolExecutionResult {
            Ok(call.arguments)
        }
    }

    struct FailingToolExecutor;

    impl ToolExecutor for FailingToolExecutor {
        async fn execute(&self, _call: ToolCall) -> ToolExecutionResult {
            Err(ToolExecutionError::UnknownTool {
                name: "missing".to_string(),
            })
        }
    }

    fn response() -> Response {
        Response {
            status: StatusCode::OK,
            body: serde_json::Value::Null,
        }
    }

    fn call_result(response: LlmResponse) -> LlmCallResult {
        LlmCallResult {
            response,
            usage: RawUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
        }
    }

    fn tool_call(id: &str, name: &str, arguments: serde_json::Value) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments,
            provider_state: None,
        }
    }

    fn tools() -> Vec<ToolSpec> {
        vec![
            ToolSpec {
                name: "get_commit_diff".to_string(),
                description: "Read a commit diff".to_string(),
                parameters: json!({ "type": "object" }),
            },
            submit_check_result(),
            request_clarification(),
        ]
    }

    fn request(tools: Vec<ToolSpec>) -> AgentRequest {
        AgentRequest {
            model: "test-model".to_string(),
            conversation: Vec::new(),
            tools,
        }
    }

    #[tokio::test]
    async fn completes_from_submit_check_result() {
        let provider =
            MockProvider::new([Ok(call_result(LlmResponse::ToolCalls(vec![tool_call(
                "call-submit",
                &submit_check_result().name,
                json!({ "summary": "done", "findings": [] }),
            )])))]);
        let transport = TestTransport::new([Ok(response())]);
        let tools = tools();

        let agent = Agent::new(provider, transport, EchoToolExecutor, Console::default());
        let outcome = agent.run_loop(request(tools), 2).await.unwrap();

        let AgentOutcome::Completed(result) = outcome else {
            panic!("expected completed outcome");
        };
        assert_eq!(result.output.summary, "done");
        assert_eq!(result.usage.input_tokens, 10);
        assert_eq!(result.iterations, 1);
    }

    #[tokio::test]
    async fn returns_clarification_without_executing_other_tools() {
        let provider = MockProvider::new([Ok(call_result(LlmResponse::ToolCalls(vec![
            tool_call(
                "call-diff",
                "get_commit_diff",
                json!({ "revision": "HEAD" }),
            ),
            tool_call(
                "call-question",
                &request_clarification().name,
                json!({ "questions": ["Which deployment policy applies?"] }),
            ),
        ])))]);
        let transport = TestTransport::new([Ok(response())]);
        let tools = tools();

        let agent = Agent::new(provider, transport, FailingToolExecutor, Console::default());
        let outcome = agent.run_loop(request(tools), 2).await.unwrap();

        let AgentOutcome::ClarificationRequested(clarification) = outcome else {
            panic!("expected clarification outcome");
        };
        assert_eq!(
            clarification.questions,
            ["Which deployment policy applies?"]
        );
        assert_eq!(clarification.usage.input_tokens, 10);
        assert_eq!(clarification.iterations, 1);
    }

    #[tokio::test]
    async fn executes_tools_and_replays_results_in_order() {
        let first = tool_call("call-1", "get_commit_diff", json!({ "revision": "HEAD" }));
        let second = tool_call("call-2", "get_commit_diff", json!({ "revision": "HEAD~1" }));
        let provider = MockProvider::new([
            Ok(call_result(LlmResponse::ToolCalls(vec![
                first.clone(),
                second.clone(),
            ]))),
            Ok(call_result(LlmResponse::ToolCalls(vec![tool_call(
                "call-submit",
                &submit_check_result().name,
                json!({ "findings": [] }),
            )]))),
        ]);
        let transport = TestTransport::new([Ok(response()), Ok(response())]);
        let tools = tools();

        let agent = Agent::new(provider, transport, EchoToolExecutor, Console::default());
        agent.run_loop(request(tools), 3).await.unwrap();

        let requests = agent.provider.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[1].conversation[0],
            ConversationTurn::AssistantToolCalls(vec![first, second])
        );
        let ConversationTurn::ToolResult { call_id, result } = &requests[1].conversation[1] else {
            panic!("expected first tool result");
        };
        assert_eq!(call_id, "call-1");
        assert_eq!(result, &json!({ "revision": "HEAD" }));
        let ConversationTurn::ToolResult { call_id, result } = &requests[1].conversation[2] else {
            panic!("expected second tool result");
        };
        assert_eq!(call_id, "call-2");
        assert_eq!(result, &json!({ "revision": "HEAD~1" }));
    }

    #[tokio::test]
    async fn final_iteration_exposes_only_terminal_tools() {
        let provider = MockProvider::new([
            Ok(call_result(LlmResponse::ToolCalls(vec![tool_call(
                "call-diff",
                "get_commit_diff",
                json!({ "revision": "HEAD" }),
            )]))),
            Ok(call_result(LlmResponse::ToolCalls(vec![tool_call(
                "call-submit",
                &submit_check_result().name,
                json!({ "findings": [] }),
            )]))),
        ]);
        let transport = TestTransport::new([Ok(response()), Ok(response())]);
        let tools = tools();

        let agent = Agent::new(provider, transport, EchoToolExecutor, Console::default());
        agent.run_loop(request(tools), 2).await.unwrap();

        let requests = agent.provider.requests();
        assert!(requests[1].is_last_request);
        assert_eq!(
            requests[1]
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            [submit_check_result().name, request_clarification().name]
        );
    }

    #[tokio::test]
    async fn returns_tool_failures_to_the_model() {
        let provider = MockProvider::new([
            Ok(call_result(LlmResponse::ToolCalls(vec![tool_call(
                "call-diff",
                "get_commit_diff",
                json!({ "revision": "HEAD" }),
            )]))),
            Ok(call_result(LlmResponse::ToolCalls(vec![tool_call(
                "call-submit",
                &submit_check_result().name,
                json!({ "findings": [] }),
            )]))),
        ]);
        let transport = TestTransport::new([Ok(response()), Ok(response())]);
        let tools = tools();

        let agent = Agent::new(provider, transport, FailingToolExecutor, Console::default());
        agent.run_loop(request(tools), 3).await.unwrap();

        let ConversationTurn::ToolResult { result, .. } =
            &agent.provider.requests()[1].conversation[1]
        else {
            panic!("expected tool result");
        };
        assert_eq!(result, &json!({ "error": "unknown tool: missing" }));
    }
}
