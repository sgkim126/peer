use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::console::Console;

use super::{
    ConversationTurn, LlmCallError, LlmProvider, LlmRequest, LlmResponse, LlmTransport, RawUsage,
    ToolCall, ToolExecutionResult, ToolExecutor, ToolSpec,
};

pub struct AgentRequest {
    pub model: String,
    pub conversation: Vec<ConversationTurn>,
    pub tools: Vec<ToolSpec>,
    pub terminal_tools: Vec<ToolSpec>,
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
pub struct AgentTerminal {
    pub call: ToolCall,
    pub usage: RawUsage,
    pub iterations: u32,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct AgentCheckpoint {
    pub conversation: Vec<ConversationTurn>,
    pub iterations: u32,
}

#[derive(Debug)]
pub struct AgentFailure {
    pub error: LlmCallError,
    pub usage: RawUsage,
    pub checkpoint: AgentCheckpoint,
    pub exhausted: bool,
}

#[derive(Debug)]
pub enum AgentOutcome {
    Terminal(AgentTerminal),
    Error(AgentFailure),
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
        checkpoint: Option<AgentCheckpoint>,
    ) -> AgentOutcome {
        let AgentRequest {
            model,
            mut conversation,
            tools,
            terminal_tools,
        } = request;
        let previous_iterations = match checkpoint {
            Some(checkpoint) if checkpoint.is_valid_for(&conversation) => {
                self.console.debug(format_args!(
                    "agent resuming from checkpoint after {} iterations",
                    checkpoint.iterations
                ));
                conversation = checkpoint.conversation;
                checkpoint.iterations
            }
            Some(_) => {
                self.console
                    .debug(format_args!("ignoring invalid agent checkpoint"));
                0
            }
            None => 0,
        };
        let all_tools = tools
            .iter()
            .chain(&terminal_tools)
            .cloned()
            .collect::<Vec<_>>();
        let terminal_tool_names = terminal_tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();
        let mut usage = RawUsage::default();

        for run_iteration in 1..=max_iterations {
            let iteration = previous_iterations.saturating_add(run_iteration);
            let is_last_request = run_iteration == max_iterations;
            let request_tools = if is_last_request {
                &terminal_tools
            } else {
                &all_tools
            };
            let http_request = match self.provider.build_request(
                LlmRequest {
                    model: &model,
                    conversation: &conversation,
                    tools: request_tools,
                },
                is_last_request,
            ) {
                Ok(http_request) => http_request,
                Err(error) => {
                    return error_outcome(error, conversation, usage, iteration, false);
                }
            };
            let response = match self.transport.send(http_request).await {
                Ok(response) => response,
                Err(error) => {
                    return error_outcome(error, conversation, usage, iteration, false);
                }
            };
            let result = match self.provider.parse_response(response) {
                Ok(result) => result,
                Err(error) => {
                    return error_outcome(error, conversation, usage, iteration, false);
                }
            };
            usage += result.usage;

            let LlmResponse::ToolCalls(tool_calls) = result.response;
            self.console.debug(format_args!(
                "agent iteration {iteration} ({run_iteration}/{} this run): {}",
                max_iterations,
                tool_calls
                    .iter()
                    .map(|tool_call| tool_call.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            ));

            let called_terminal_tools = tool_calls
                .iter()
                .filter(|call| terminal_tool_names.contains(&call.name.as_str()))
                .collect::<Vec<_>>();
            match called_terminal_tools.as_slice() {
                [tool_call] => {
                    return AgentOutcome::Terminal(AgentTerminal {
                        call: (*tool_call).clone(),
                        usage,
                        iterations: iteration,
                    });
                }
                [] => {}
                calls => {
                    return error_outcome(
                        permanent_error(
                            format!(
                                "LLM agent called multiple terminal tools: {}",
                                calls
                                    .iter()
                                    .map(|call| call.name.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ),
                            AgentError::MultipleTerminalCalls,
                        ),
                        conversation,
                        usage,
                        iteration,
                        false,
                    );
                }
            }

            if !tool_calls.is_empty() {
                conversation.push(ConversationTurn::AssistantToolCalls(tool_calls.clone()));
                for (call_id, result) in self.tool_executor.execute_all(tool_calls).await {
                    conversation.push(ConversationTurn::ToolResult {
                        call_id,
                        result: tool_result_json(result),
                    });
                }
            }

            if is_last_request {
                return error_outcome(
                    permanent_error(
                        format!(
                            "LLM agent did not call a terminal tool within {iteration} iterations"
                        ),
                        AgentError::LoopExhausted,
                    ),
                    conversation,
                    usage,
                    iteration,
                    true,
                );
            }
        }
        let iterations = previous_iterations.saturating_add(max_iterations);
        error_outcome(
            permanent_error(
                format!("LLM agent did not call a terminal tool within {iterations} iterations"),
                AgentError::LoopExhausted,
            ),
            conversation,
            usage,
            iterations,
            true,
        )
    }
}

impl AgentCheckpoint {
    fn is_valid_for(&self, initial_conversation: &[ConversationTurn]) -> bool {
        if !self.conversation.starts_with(initial_conversation) {
            return false;
        }

        let mut remaining = &self.conversation[initial_conversation.len()..];
        while let Some((turn, rest)) = remaining.split_first() {
            let ConversationTurn::AssistantToolCalls(calls) = turn else {
                return false;
            };
            if calls.is_empty() || rest.len() < calls.len() {
                return false;
            }
            if !calls.iter().zip(rest).all(|(call, turn)| {
                matches!(
                    turn,
                    ConversationTurn::ToolResult { call_id, .. } if call_id == &call.id
                )
            }) {
                return false;
            }
            remaining = &rest[calls.len()..];
        }
        true
    }
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

fn error_outcome(
    error: LlmCallError,
    conversation: Vec<ConversationTurn>,
    usage: RawUsage,
    iterations: u32,
    exhausted: bool,
) -> AgentOutcome {
    AgentOutcome::Error(AgentFailure {
        error,
        usage,
        checkpoint: AgentCheckpoint {
            conversation,
            iterations,
        },
        exhausted,
    })
}

#[derive(Debug)]
enum AgentError {
    MultipleTerminalCalls,
    LoopExhausted,
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MultipleTerminalCalls => f.write_str("multiple terminal tool calls"),
            Self::LoopExhausted => f.write_str("agent loop exhausted"),
        }
    }
}

impl std::error::Error for AgentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::VecDeque;
    use std::sync::Mutex;

    use reqwest::StatusCode;

    use super::super::{
        LlmCallResult, MockProvider, Request, Response, ToolExecutionError, request_clarification,
        submit_check_result,
    };

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
        vec![ToolSpec {
            name: "get_commit_diff".to_string(),
            description: "Read a commit diff".to_string(),
            parameters: json!({ "type": "object" }),
        }]
    }

    fn request(tools: Vec<ToolSpec>) -> AgentRequest {
        AgentRequest {
            model: "test-model".to_string(),
            conversation: Vec::new(),
            tools,
            terminal_tools: vec![request_clarification(), submit_check_result()],
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
        let outcome = agent.run_loop(request(tools), 2, None).await;

        let AgentOutcome::Terminal(result) = outcome else {
            panic!("expected terminal outcome");
        };
        assert_eq!(result.call.name, submit_check_result().name);
        assert_eq!(
            result.call.arguments,
            json!({ "summary": "done", "findings": [] })
        );
        assert_eq!(result.usage.input_tokens, 10);
        assert_eq!(result.iterations, 1);
    }

    #[tokio::test]
    async fn completes_from_a_custom_terminal_tool() {
        let completion_tool = ToolSpec {
            name: "submit_custom_output".to_string(),
            description: "Submit custom output".to_string(),
            parameters: json!({ "type": "object" }),
        };
        let provider =
            MockProvider::new([Ok(call_result(LlmResponse::ToolCalls(vec![tool_call(
                "call-submit",
                &completion_tool.name,
                json!({ "value": "done" }),
            )])))]);
        let transport = TestTransport::new([Ok(response())]);
        let request = AgentRequest {
            model: "test-model".to_string(),
            conversation: Vec::new(),
            tools: Vec::new(),
            terminal_tools: vec![completion_tool],
        };

        let agent = Agent::new(provider, transport, EchoToolExecutor, Console::default());
        let outcome = agent.run_loop(request, 1, None).await;

        let AgentOutcome::Terminal(result) = outcome else {
            panic!("expected terminal outcome");
        };
        assert_eq!(result.call.name, "submit_custom_output");
        assert_eq!(result.call.arguments, json!({ "value": "done" }));
    }

    #[tokio::test]
    async fn does_not_parse_terminal_tool_arguments() {
        let provider =
            MockProvider::new([Ok(call_result(LlmResponse::ToolCalls(vec![tool_call(
                "call-submit",
                &submit_check_result().name,
                json!({ "findings": [{}] }),
            )])))]);
        let transport = TestTransport::new([Ok(response())]);
        let agent = Agent::new(provider, transport, EchoToolExecutor, Console::default());

        let outcome = agent.run_loop(request(tools()), 2, None).await;

        let AgentOutcome::Terminal(result) = outcome else {
            panic!("expected terminal outcome");
        };
        assert_eq!(result.call.arguments, json!({ "findings": [{}] }));
        assert_eq!(result.usage.input_tokens, 10);
        assert_eq!(result.usage.output_tokens, 5);
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
        let outcome = agent.run_loop(request(tools), 2, None).await;

        let AgentOutcome::Terminal(terminal) = outcome else {
            panic!("expected terminal outcome");
        };
        assert_eq!(terminal.call.name, request_clarification().name);
        assert_eq!(
            terminal.call.arguments,
            json!({ "questions": ["Which deployment policy applies?"] })
        );
        assert_eq!(terminal.usage.input_tokens, 10);
        assert_eq!(terminal.iterations, 1);
    }

    #[tokio::test]
    async fn rejects_multiple_terminal_tool_calls() {
        let provider = MockProvider::new([Ok(call_result(LlmResponse::ToolCalls(vec![
            tool_call(
                "call-question",
                &request_clarification().name,
                json!({ "questions": ["Which policy applies?"] }),
            ),
            tool_call(
                "call-submit",
                &submit_check_result().name,
                json!({ "findings": [] }),
            ),
        ])))]);
        let transport = TestTransport::new([Ok(response())]);

        let agent = Agent::new(provider, transport, EchoToolExecutor, Console::default());
        let outcome = agent.run_loop(request(tools()), 2, None).await;

        let AgentOutcome::Error(failure) = outcome else {
            panic!("expected error outcome");
        };
        assert_eq!(
            failure.error.to_string(),
            "permanent LLM call failure: LLM agent called multiple terminal tools: request_clarification, submit_check_result"
        );
        assert_eq!(failure.usage.input_tokens, 10);
        assert_eq!(failure.checkpoint.iterations, 1);
    }

    #[tokio::test]
    async fn transient_failure_preserves_the_current_conversation() {
        let conversation = vec![
            ConversationTurn::System("Review code.".to_string()),
            ConversationTurn::User("Review abc1234.".to_string()),
        ];
        let provider = MockProvider::default();
        let transport = TestTransport::new([Err(LlmCallError::Transient {
            message: "request timed out".to_string(),
            source: Box::new(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "request timed out",
            )),
        })]);
        let agent = Agent::new(provider, transport, EchoToolExecutor, Console::default());
        let request = AgentRequest {
            model: "test-model".to_string(),
            conversation: conversation.clone(),
            tools: tools(),
            terminal_tools: vec![submit_check_result()],
        };

        let AgentOutcome::Error(failure) = agent.run_loop(request, 2, None).await else {
            panic!("expected error outcome");
        };

        assert_eq!(failure.checkpoint.conversation, conversation);
        assert_eq!(failure.checkpoint.iterations, 1);
        assert!(!failure.exhausted);
    }

    #[tokio::test]
    async fn exhaustion_preserves_the_last_tool_exchange() {
        let call = tool_call(
            "call-diff",
            "get_commit_diff",
            json!({ "revision": "HEAD" }),
        );
        let provider =
            MockProvider::new([Ok(call_result(LlmResponse::ToolCalls(vec![call.clone()])))]);
        let transport = TestTransport::new([Ok(response())]);
        let agent = Agent::new(provider, transport, EchoToolExecutor, Console::default());

        let AgentOutcome::Error(failure) = agent.run_loop(request(tools()), 1, None).await else {
            panic!("expected error outcome");
        };

        assert!(failure.exhausted);
        assert_eq!(failure.checkpoint.iterations, 1);
        assert_eq!(
            failure.checkpoint.conversation,
            [
                ConversationTurn::AssistantToolCalls(vec![call]),
                ConversationTurn::ToolResult {
                    call_id: "call-diff".to_string(),
                    result: json!({ "revision": "HEAD" }),
                },
            ]
        );
    }

    #[test]
    fn checkpoint_round_trips_through_json() {
        let checkpoint = AgentCheckpoint {
            conversation: vec![ConversationTurn::AssistantToolCalls(vec![ToolCall {
                id: "call-1".to_string(),
                name: "get_commit_diff".to_string(),
                arguments: json!({ "revision": "HEAD" }),
                provider_state: Some("opaque-state".to_string()),
            }])],
            iterations: 2,
        };

        let value = serde_json::to_value(&checkpoint).unwrap();
        let restored: AgentCheckpoint = serde_json::from_value(value.clone()).unwrap();

        assert_eq!(restored, checkpoint);
        assert_eq!(
            value["conversation"][0]["type"],
            serde_json::Value::String("assistant_tool_calls".to_string())
        );
        assert_eq!(
            value["conversation"][0]["content"][0]["provider_state"],
            "opaque-state"
        );
    }

    #[tokio::test]
    async fn resumes_a_valid_checkpoint_with_a_fresh_iteration_budget() {
        let initial = vec![
            ConversationTurn::System("Review code.".to_string()),
            ConversationTurn::User("Review abc1234.".to_string()),
        ];
        let previous_call = tool_call(
            "call-diff",
            "get_commit_diff",
            json!({ "revision": "abc1234" }),
        );
        let mut saved_conversation = initial.clone();
        saved_conversation.push(ConversationTurn::AssistantToolCalls(vec![
            previous_call.clone(),
        ]));
        saved_conversation.push(ConversationTurn::ToolResult {
            call_id: previous_call.id.clone(),
            result: previous_call.arguments.clone(),
        });
        let checkpoint = AgentCheckpoint {
            conversation: saved_conversation.clone(),
            iterations: 3,
        };
        let provider =
            MockProvider::new([Ok(call_result(LlmResponse::ToolCalls(vec![tool_call(
                "call-submit",
                &submit_check_result().name,
                json!({ "summary": "done", "findings": [] }),
            )])))]);
        let transport = TestTransport::new([Ok(response())]);
        let agent = Agent::new(provider, transport, EchoToolExecutor, Console::default());
        let request = AgentRequest {
            model: "test-model".to_string(),
            conversation: initial,
            tools: tools(),
            terminal_tools: vec![submit_check_result()],
        };

        let AgentOutcome::Terminal(terminal) = agent.run_loop(request, 2, Some(checkpoint)).await
        else {
            panic!("expected terminal outcome");
        };

        assert_eq!(
            agent.provider.requests()[0].conversation,
            saved_conversation
        );
        assert_eq!(terminal.iterations, 4);
        assert_eq!(
            terminal.usage,
            RawUsage {
                input_tokens: 10,
                output_tokens: 5,
            }
        );
    }

    #[tokio::test]
    async fn ignores_a_checkpoint_for_different_initial_conversation() {
        let initial = vec![ConversationTurn::System("Current prompt.".to_string())];
        let checkpoint = AgentCheckpoint {
            conversation: vec![ConversationTurn::System("Old prompt.".to_string())],
            iterations: 4,
        };
        let provider =
            MockProvider::new([Ok(call_result(LlmResponse::ToolCalls(vec![tool_call(
                "call-submit",
                &submit_check_result().name,
                json!({ "summary": "done", "findings": [] }),
            )])))]);
        let transport = TestTransport::new([Ok(response())]);
        let agent = Agent::new(provider, transport, EchoToolExecutor, Console::default());
        let request = AgentRequest {
            model: "test-model".to_string(),
            conversation: initial.clone(),
            tools: tools(),
            terminal_tools: vec![submit_check_result()],
        };

        let AgentOutcome::Terminal(terminal) = agent.run_loop(request, 2, Some(checkpoint)).await
        else {
            panic!("expected terminal outcome");
        };

        assert_eq!(agent.provider.requests()[0].conversation, initial);
        assert_eq!(terminal.iterations, 1);
    }

    #[test]
    fn rejects_a_checkpoint_with_an_incomplete_tool_exchange() {
        let initial = vec![ConversationTurn::System("Review code.".to_string())];
        let checkpoint = AgentCheckpoint {
            conversation: vec![
                initial[0].clone(),
                ConversationTurn::AssistantToolCalls(vec![tool_call(
                    "call-diff",
                    "get_commit_diff",
                    json!({ "revision": "HEAD" }),
                )]),
            ],
            iterations: 1,
        };

        assert!(!checkpoint.is_valid_for(&initial));
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
        agent.run_loop(request(tools), 3, None).await;

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
        agent.run_loop(request(tools), 2, None).await;

        let requests = agent.provider.requests();
        assert!(requests[1].is_last_request);
        assert_eq!(
            requests[1]
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            [request_clarification().name, submit_check_result().name]
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
        agent.run_loop(request(tools), 3, None).await;

        let ConversationTurn::ToolResult { result, .. } =
            &agent.provider.requests()[1].conversation[1]
        else {
            panic!("expected tool result");
        };
        assert_eq!(result, &json!({ "error": "unknown tool: missing" }));
    }
}
