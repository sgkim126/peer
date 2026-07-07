use std::collections::VecDeque;
use std::sync::Mutex;

use super::agent::{ToolExecutionResult, ToolExecutor};
use super::provider::{
    ConversationTurn, LlmCallError, LlmCallResult, LlmOutputMode, LlmProvider, LlmRequest,
    ToolCall, ToolSpec,
};

#[derive(Debug, Clone, PartialEq)]
pub struct RecordedLlmRequest {
    pub model: String,
    pub conversation: Vec<ConversationTurn>,
    pub output_mode: RecordedLlmOutputMode,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RecordedLlmOutputMode {
    Check {
        tools: Vec<ToolSpec>,
        output_schema: serde_json::Value,
    },
    Text,
}

#[derive(Debug)]
pub struct MockProvider {
    responses: Mutex<VecDeque<Result<LlmCallResult, LlmCallError>>>,
    requests: Mutex<Vec<RecordedLlmRequest>>,
}

#[derive(Debug)]
pub struct FakeToolExecutor {
    responses: Mutex<VecDeque<ToolExecutionResult>>,
    calls: Mutex<Vec<ToolCall>>,
}

impl MockProvider {
    pub fn new(responses: impl IntoIterator<Item = Result<LlmCallResult, LlmCallError>>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().collect()),
            requests: Mutex::new(Vec::new()),
        }
    }

    pub fn requests(&self) -> Vec<RecordedLlmRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl Default for MockProvider {
    fn default() -> Self {
        Self::new([])
    }
}

impl FakeToolExecutor {
    pub fn new(responses: impl IntoIterator<Item = ToolExecutionResult>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().collect()),
            calls: Mutex::new(Vec::new()),
        }
    }

    pub fn calls(&self) -> Vec<ToolCall> {
        self.calls.lock().unwrap().clone()
    }
}

impl Default for FakeToolExecutor {
    fn default() -> Self {
        Self::new([])
    }
}

impl LlmProvider for MockProvider {
    async fn send(&self, request: LlmRequest<'_>) -> Result<LlmCallResult, LlmCallError> {
        self.requests.lock().unwrap().push(RecordedLlmRequest {
            model: request.model.to_string(),
            conversation: request.conversation.to_vec(),
            output_mode: match request.output_mode {
                LlmOutputMode::Check {
                    tools,
                    output_schema,
                } => RecordedLlmOutputMode::Check {
                    tools: tools.to_vec(),
                    output_schema: output_schema.clone(),
                },
                LlmOutputMode::Text => RecordedLlmOutputMode::Text,
            },
        });

        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| panic!("MockProvider has no queued response"))
    }
}

impl ToolExecutor for FakeToolExecutor {
    async fn execute(&self, call: ToolCall) -> ToolExecutionResult {
        self.calls.lock().unwrap().push(call);

        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| panic!("FakeToolExecutor has no queued response"))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::llm::confidence::Confidence;
    use crate::llm::provider::{LlmResponse, RawUsage};
    use crate::llm::result::CheckOutput;

    fn check_result(summary: &str) -> LlmCallResult {
        LlmCallResult {
            response: LlmResponse::CheckOutput(CheckOutput {
                summary: summary.to_string(),
                findings: Vec::new(),
                confidence: Confidence::try_from(0.9).unwrap(),
            }),
            usage: RawUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
        }
    }

    #[tokio::test]
    async fn returns_queued_responses_in_order() {
        let provider = MockProvider::new([Ok(check_result("first")), Ok(check_result("second"))]);
        let schema = json!({
            "type": "object"
        });
        let first_request = LlmRequest {
            model: "test-model",
            conversation: &[],
            output_mode: LlmOutputMode::Check {
                tools: &[],
                output_schema: &schema,
            },
        };
        let second_request = LlmRequest {
            model: "test-model",
            conversation: &[],
            output_mode: LlmOutputMode::Check {
                tools: &[],
                output_schema: &schema,
            },
        };

        let first = provider.send(first_request).await.unwrap();
        let second = provider.send(second_request).await.unwrap();

        let LlmResponse::CheckOutput(first) = first.response else {
            panic!("expected check output");
        };
        let LlmResponse::CheckOutput(second) = second.response else {
            panic!("expected check output");
        };
        assert_eq!(first.summary, "first");
        assert_eq!(second.summary, "second");
    }

    #[tokio::test]
    async fn records_requests() {
        let provider = MockProvider::new([Ok(check_result("ok"))]);
        let conversation = [ConversationTurn::System("system prompt".to_string())];
        let tools = [ToolSpec {
            name: "commit_diff".to_string(),
            description: "Read a commit diff".to_string(),
            parameters: json!({
                "type": "object"
            }),
        }];
        let schema = json!({
            "type": "object"
        });
        let request = LlmRequest {
            model: "test-model",
            conversation: &conversation,
            output_mode: LlmOutputMode::Check {
                tools: &tools,
                output_schema: &schema,
            },
        };

        provider.send(request).await.unwrap();

        let requests = provider.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].model, "test-model");
        assert_eq!(requests[0].conversation, conversation);
        assert_eq!(
            requests[0].output_mode,
            RecordedLlmOutputMode::Check {
                tools: tools.to_vec(),
                output_schema: schema,
            }
        );
    }

    #[tokio::test]
    async fn records_text_requests() {
        let provider = MockProvider::new([Ok(LlmCallResult {
            response: LlmResponse::Text("summary".to_string()),
            usage: RawUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
        })]);
        let request = LlmRequest {
            model: "test-model",
            conversation: &[],
            output_mode: LlmOutputMode::Text,
        };

        provider.send(request).await.unwrap();

        assert_eq!(
            provider.requests()[0].output_mode,
            RecordedLlmOutputMode::Text
        );
    }

    #[tokio::test]
    #[should_panic(expected = "MockProvider has no queued response")]
    async fn panics_when_no_response_is_queued() {
        let provider = MockProvider::default();
        let schema = json!({
            "type": "object"
        });
        let request = LlmRequest {
            model: "test-model",
            conversation: &[],
            output_mode: LlmOutputMode::Check {
                tools: &[],
                output_schema: &schema,
            },
        };

        let _ = provider.send(request).await;
    }

    #[tokio::test]
    async fn fake_tool_executor_returns_queued_results_and_records_calls() {
        let executor = FakeToolExecutor::new([Ok(json!({
            "ok": true
        }))]);
        let call = ToolCall {
            id: "call-1".to_string(),
            name: "commit_diff".to_string(),
            arguments: json!({
                "hash": "abc1234"
            }),
        };

        let result = executor.execute(call.clone()).await;

        assert_eq!(
            result.unwrap(),
            json!({
                "ok": true
            })
        );
        assert_eq!(executor.calls(), vec![call]);
    }
}
