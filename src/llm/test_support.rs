use std::collections::VecDeque;
use std::sync::Mutex;

use reqwest::header::HeaderMap;

use super::provider::{
    ConversationTurn, LlmCallError, LlmCallResult, LlmProvider, LlmRequest, Request, Response,
    ToolSpec,
};

#[derive(Debug, Clone, PartialEq)]
pub struct RecordedLlmRequest {
    pub model: String,
    pub conversation: Vec<ConversationTurn>,
    pub tools: Vec<ToolSpec>,
    pub is_last_request: bool,
}

#[derive(Debug)]
pub struct MockProvider {
    responses: Mutex<VecDeque<Result<LlmCallResult, LlmCallError>>>,
    requests: Mutex<Vec<RecordedLlmRequest>>,
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

impl LlmProvider for MockProvider {
    fn build_request(
        &self,
        request: LlmRequest<'_>,
        is_last_request: bool,
    ) -> Result<Request, LlmCallError> {
        self.requests.lock().unwrap().push(RecordedLlmRequest {
            model: request.model.to_string(),
            conversation: request.conversation.to_vec(),
            tools: request.tools.to_vec(),
            is_last_request,
        });

        Ok(Request {
            url: "mock://provider".to_string(),
            headers: HeaderMap::new(),
            body: serde_json::Value::Null,
        })
    }

    fn parse_response(&self, _response: Response) -> Result<LlmCallResult, LlmCallError> {
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| panic!("MockProvider has no queued response"))
    }
}

#[cfg(test)]
mod tests {
    use reqwest::StatusCode;
    use serde_json::json;

    use super::*;
    use crate::llm::provider::{LlmResponse, RawUsage, ToolCall};

    fn tool_call_result(id: &str) -> LlmCallResult {
        LlmCallResult {
            response: LlmResponse::ToolCalls(vec![ToolCall {
                id: id.to_string(),
                name: "test_tool".to_string(),
                arguments: json!({
                    "id": id
                }),
                thought_signature: None,
            }]),
            usage: RawUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
        }
    }

    fn response() -> Response {
        Response {
            status: StatusCode::OK,
            body: serde_json::Value::Null,
        }
    }

    #[test]
    fn returns_queued_responses_in_order() {
        let provider = MockProvider::new([
            Ok(tool_call_result("first")),
            Ok(tool_call_result("second")),
        ]);

        let first = provider.parse_response(response()).unwrap();
        let second = provider.parse_response(response()).unwrap();

        let LlmResponse::ToolCalls(first) = first.response;
        let LlmResponse::ToolCalls(second) = second.response;
        assert_eq!(first[0].id, "first");
        assert_eq!(second[0].id, "second");
    }

    #[test]
    fn records_requests() {
        let provider = MockProvider::default();
        let conversation = [ConversationTurn::System("system prompt".to_string())];
        let tools = [ToolSpec {
            name: "commit_diff".to_string(),
            description: "Read a commit diff".to_string(),
            parameters: json!({
                "type": "object"
            }),
        }];
        let request = LlmRequest {
            model: "test-model",
            conversation: &conversation,
            tools: &tools,
        };

        provider.build_request(request, true).unwrap();

        let requests = provider.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].model, "test-model");
        assert_eq!(requests[0].conversation, conversation);
        assert_eq!(requests[0].tools, tools);
        assert!(requests[0].is_last_request);
    }

    #[test]
    #[should_panic(expected = "MockProvider has no queued response")]
    fn panics_when_no_response_is_queued() {
        let provider = MockProvider::default();

        let _ = provider.parse_response(response());
    }
}
