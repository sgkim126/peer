use std::fmt;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::json;

use crate::secret::Secret;

use super::{
    ConversationTurn, LlmCallError, LlmCallResult, LlmProvider, LlmRequest, LlmResponse, RawUsage,
    Request, Response, ToolCall, ToolSpec,
};

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const DEFAULT_MAX_TOKENS: u32 = 4096;
const ANTHROPIC_VERSION: &str = "2023-06-01";
const API_KEY_HEADER: HeaderName = HeaderName::from_static("x-api-key");
const VERSION_HEADER: HeaderName = HeaderName::from_static("anthropic-version");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnthropicProvider {
    api_key: Secret,
    base_url: String,
}

impl AnthropicProvider {
    pub fn from_env(api_key_env: &str, base_url: Option<&str>) -> Result<Self, LlmCallError> {
        let api_key = Secret::from_env(api_key_env).map_err(|error| LlmCallError::Permanent {
            message: format!("cannot read {api_key_env}"),
            source: Box::new(error),
        })?;

        Ok(Self::new(api_key, base_url))
    }

    fn new(api_key: Secret, base_url: Option<&str>) -> Self {
        Self {
            api_key,
            base_url: base_url
                .unwrap_or(DEFAULT_BASE_URL)
                .trim_end_matches('/')
                .to_string(),
        }
    }
}

impl LlmProvider for AnthropicProvider {
    fn build_request(
        &self,
        request: LlmRequest<'_>,
        is_last_request: bool,
    ) -> Result<Request, LlmCallError> {
        let mut api_key = self
            .api_key
            .expose_secret()
            .parse::<HeaderValue>()
            .map_err(|error| LlmCallError::Permanent {
                message: "failed to build Anthropic API key header".to_string(),
                source: Box::new(error),
            })?;
        api_key.set_sensitive(true);

        let mut headers = HeaderMap::new();
        headers.insert(API_KEY_HEADER, api_key);
        headers.insert(VERSION_HEADER, HeaderValue::from_static(ANTHROPIC_VERSION));

        Ok(Request {
            url: format!("{}/v1/messages", self.base_url),
            headers,
            body: request_body(request, is_last_request)?,
        })
    }

    fn parse_response(&self, response: Response) -> Result<LlmCallResult, LlmCallError> {
        if response.status.is_success() {
            parse_success(&response.body)
        } else {
            Err(parse_error(response.status.as_u16(), &response.body))
        }
    }
}

fn request_body(
    request: LlmRequest<'_>,
    is_last_request: bool,
) -> Result<serde_json::Value, LlmCallError> {
    let mut system = Vec::new();
    let mut messages = Vec::new();

    for turn in request.conversation {
        match turn {
            ConversationTurn::System(content) => system.push(content.as_str()),
            ConversationTurn::User(content) => messages.push(json!({
                "role": "user",
                "content": content,
            })),
            ConversationTurn::AssistantToolCalls(tool_calls) => messages.push(json!({
                "role": "assistant",
                "content": tool_calls.iter().map(tool_use).collect::<Vec<_>>(),
            })),
            ConversationTurn::ToolResult { call_id, result } => messages.push(json!({
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": call_id,
                    "content": result.to_string(),
                }],
            })),
        }
    }
    let tools: Vec<_> = request.tools.iter().map(tool).collect();

    Ok(json!({
        "model": request.model,
        "max_tokens": DEFAULT_MAX_TOKENS,
        "system": system.join("\n\n"),
        "messages": messages,
        "tools": tools,
        "tool_choice": if is_last_request {
            json!({
                "type": "any",
                "disable_parallel_tool_use": true,
            })
        } else {
            json!({
                "type": "any"
            })
        },
    }))
}

fn tool_use(tool_call: &ToolCall) -> serde_json::Value {
    json!({
        "type": "tool_use",
        "id": tool_call.id,
        "name": tool_call.name,
        "input": tool_call.arguments,
    })
}

fn tool(spec: &ToolSpec) -> serde_json::Value {
    json!({
        "name": spec.name,
        "description": spec.description,
        "input_schema": spec.parameters,
    })
}

fn parse_success(body: &serde_json::Value) -> Result<LlmCallResult, LlmCallError> {
    if body
        .pointer("/stop_reason")
        .and_then(serde_json::Value::as_str)
        == Some("model_context_window_exceeded")
    {
        return Err(LlmCallError::ContextOverflow {
            message: "model context window exceeded during generation".to_string(),
        });
    }

    let content = body
        .get("content")
        .and_then(serde_json::Value::as_array)
        .ok_or(permanent_parse_error("missing content array"))?;
    let usage = RawUsage {
        input_tokens: required_u64(body, "/usage/input_tokens")?,
        output_tokens: required_u64(body, "/usage/output_tokens")?,
    };
    let tool_calls = content
        .iter()
        .filter(|block| block.get("type").and_then(serde_json::Value::as_str) == Some("tool_use"))
        .map(parse_tool_call)
        .collect::<Result<Vec<_>, _>>()?;

    if tool_calls.is_empty() {
        return Err(permanent_parse_error("missing tool_use content block"));
    }

    Ok(LlmCallResult {
        response: LlmResponse::ToolCalls(tool_calls),
        usage,
    })
}

fn parse_tool_call(value: &serde_json::Value) -> Result<ToolCall, LlmCallError> {
    let id = required_string(value, "/id")?.to_string();
    let name = required_string(value, "/name")?.to_string();
    let arguments = value
        .get("input")
        .cloned()
        .ok_or(permanent_parse_error("missing tool input"))?;

    Ok(ToolCall {
        id,
        name,
        arguments,
        provider_state: None,
    })
}

fn required_string<'a>(
    value: &'a serde_json::Value,
    pointer: &str,
) -> Result<&'a str, LlmCallError> {
    value
        .pointer(pointer)
        .and_then(serde_json::Value::as_str)
        .ok_or(permanent_parse_error(format!(
            "missing string at {pointer}"
        )))
}

fn required_u64(value: &serde_json::Value, pointer: &str) -> Result<u64, LlmCallError> {
    value
        .pointer(pointer)
        .and_then(serde_json::Value::as_u64)
        .ok_or(permanent_parse_error(format!(
            "missing unsigned integer at {pointer}"
        )))
}

fn parse_error(status: u16, body: &serde_json::Value) -> LlmCallError {
    let message = body
        .pointer("/error/message")
        .or_else(|| body.pointer("/message"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("Anthropic request failed")
        .to_string();

    if is_context_overflow(status, body, &message) {
        return LlmCallError::ContextOverflow { message };
    }

    let source = Box::new(AnthropicStatusError {
        status,
        message: message.clone(),
    });
    if is_transient_status(status) {
        LlmCallError::Transient { message, source }
    } else {
        LlmCallError::Permanent { message, source }
    }
}

fn is_context_overflow(status: u16, body: &serde_json::Value, message: &str) -> bool {
    if status != 400 {
        return false;
    }

    let is_invalid_request = body
        .pointer("/error/type")
        .and_then(serde_json::Value::as_str)
        == Some("invalid_request_error");
    let message = message.to_ascii_lowercase();

    if is_invalid_request && message.contains("prompt is too long") {
        return true;
    }

    let identifies_context =
        message.contains("context window") || message.contains("context length");
    let indicates_overflow =
        message.contains("exceed") || message.contains("maximum") || message.contains("too long");

    identifies_context && indicates_overflow
}

fn is_transient_status(status: u16) -> bool {
    matches!(status, 408 | 409 | 425 | 429 | 500 | 502 | 503 | 504)
}

fn permanent_parse_error(message: impl Into<String>) -> LlmCallError {
    let message = message.into();
    LlmCallError::Permanent {
        source: Box::new(AnthropicParseError(message.clone())),
        message,
    }
}

#[derive(Debug)]
struct AnthropicParseError(String);

impl fmt::Display for AnthropicParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for AnthropicParseError {}

#[derive(Debug)]
struct AnthropicStatusError {
    status: u16,
    message: String,
}

impl fmt::Display for AnthropicStatusError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Anthropic returned HTTP {}: {}",
            self.status, self.message
        )
    }
}

impl std::error::Error for AnthropicStatusError {}

#[cfg(test)]
mod tests {
    use super::*;

    use std::assert_matches;

    use reqwest::StatusCode;

    fn provider() -> AnthropicProvider {
        AnthropicProvider::new(
            Secret::new("test-api-key".to_string()),
            Some("https://anthropic.example.test/"),
        )
    }

    fn test_tool() -> ToolSpec {
        ToolSpec {
            name: "test_tool".to_string(),
            description: "A test tool".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string"
                    },
                },
                "required": ["path"],
            }),
        }
    }

    #[test]
    fn builds_request_with_headers_messages_and_tools() {
        let parameters = json!({
            "type": "object",
            "properties": {
                "hash": {
                    "type": "string"
                },
            },
            "required": ["hash"],
        });
        let tools = [
            ToolSpec {
                name: "commit_diff".to_string(),
                description: "Read a commit diff".to_string(),
                parameters: parameters.clone(),
            },
            test_tool(),
        ];
        let conversation = [
            ConversationTurn::System("You review code.".to_string()),
            ConversationTurn::User("Check abc1234.".to_string()),
        ];

        let request = provider()
            .build_request(
                LlmRequest {
                    model: "claude-sonnet-4-5",
                    conversation: &conversation,
                    tools: &tools,
                },
                false,
            )
            .unwrap();

        assert_eq!(request.url, "https://anthropic.example.test/v1/messages");
        assert_eq!(request.headers[API_KEY_HEADER], "test-api-key");
        assert!(request.headers[API_KEY_HEADER].is_sensitive());
        assert_eq!(request.headers[VERSION_HEADER], ANTHROPIC_VERSION);
        assert_eq!(request.body["model"], "claude-sonnet-4-5");
        assert_eq!(request.body["max_tokens"], DEFAULT_MAX_TOKENS);
        assert_eq!(request.body["system"], "You review code.");
        assert_eq!(request.body["messages"][0]["role"], "user");
        assert_eq!(request.body["messages"][0]["content"], "Check abc1234.");
        assert_eq!(request.body["tools"][0]["name"], "commit_diff");
        assert_eq!(request.body["tools"][0]["input_schema"], parameters);
        assert_eq!(request.body["tools"][1]["name"], "test_tool");
        assert_eq!(
            request.body["tool_choice"],
            json!({
                "type": "any"
            })
        );
    }

    #[test]
    fn last_request_requires_one_non_parallel_tool_call() {
        let tools = [test_tool()];

        let request = provider()
            .build_request(
                LlmRequest {
                    model: "claude-sonnet-4-5",
                    conversation: &[],
                    tools: &tools,
                },
                true,
            )
            .unwrap();

        assert_eq!(
            request.body["tool_choice"],
            json!({
                "type": "any",
                "disable_parallel_tool_use": true,
            })
        );
    }

    #[test]
    fn joins_system_turns_and_encodes_tool_calls_and_results() {
        let conversation = [
            ConversationTurn::System("You review code.".to_string()),
            ConversationTurn::System("Be concise.".to_string()),
            ConversationTurn::AssistantToolCalls(vec![ToolCall {
                id: "call-1".to_string(),
                name: "commit_diff".to_string(),
                arguments: json!({
                    "hash": "abc1234"
                }),
                provider_state: None,
            }]),
            ConversationTurn::ToolResult {
                call_id: "call-1".to_string(),
                result: json!({
                    "diff": "+hello"
                }),
            },
        ];

        let request = provider()
            .build_request(
                LlmRequest {
                    model: "claude-sonnet-4-5",
                    conversation: &conversation,
                    tools: &[],
                },
                false,
            )
            .unwrap();

        assert_eq!(request.body["system"], "You review code.\n\nBe concise.");
        assert_eq!(request.body["messages"][0]["role"], "assistant");
        assert_eq!(
            request.body["messages"][0]["content"][0]["type"],
            "tool_use"
        );
        assert_eq!(
            request.body["messages"][0]["content"][0]["input"],
            json!({
                "hash": "abc1234"
            })
        );
        assert_eq!(request.body["messages"][1]["role"], "user");
        assert_eq!(
            request.body["messages"][1]["content"][0]["type"],
            "tool_result"
        );
        assert_eq!(
            request.body["messages"][1]["content"][0]["tool_use_id"],
            "call-1"
        );
        assert_eq!(
            request.body["messages"][1]["content"][0]["content"],
            "{\"diff\":\"+hello\"}"
        );
    }

    #[test]
    fn request_debug_redacts_api_key() {
        let request = provider()
            .build_request(
                LlmRequest {
                    model: "claude-sonnet-4-5",
                    conversation: &[],
                    tools: &[],
                },
                false,
            )
            .unwrap();

        assert!(!format!("{request:?}").contains("test-api-key"));
    }

    #[test]
    fn missing_api_key_is_permanent_error() {
        let name = "PEER_TEST_MISSING_ANTHROPIC_API_KEY_4B9D5E7C9A1F";

        let error = AnthropicProvider::from_env(name, None).unwrap_err();

        assert_matches!(error, LlmCallError::Permanent { .. });
        assert!(error.to_string().contains(name));
    }

    #[test]
    fn parses_tool_use_response() {
        let response = Response {
            status: StatusCode::OK,
            body: json!({
                "content": [{
                    "type": "tool_use",
                    "id": "call-1",
                    "name": "commit_diff",
                    "input": { "hash": "abc1234" }
                }],
                "usage": {
                    "input_tokens": 100,
                    "output_tokens": 25
                }
            }),
        };

        let result = provider().parse_response(response).unwrap();

        assert_eq!(result.usage.input_tokens, 100);
        assert_eq!(result.usage.output_tokens, 25);
        let LlmResponse::ToolCalls(tool_calls) = result.response;
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "call-1");
        assert_eq!(tool_calls[0].name, "commit_diff");
        assert_eq!(
            tool_calls[0].arguments,
            json!({
                "hash": "abc1234"
            })
        );
        assert_eq!(tool_calls[0].provider_state, None);
    }

    #[test]
    fn rejects_response_without_tool_use() {
        let response = Response {
            status: StatusCode::OK,
            body: json!({
                "content": [{
                    "type": "text",
                    "text": "No tools needed."
                }],
                "usage": {
                    "input_tokens": 10,
                    "output_tokens": 5
                }
            }),
        };

        let error = provider().parse_response(response).unwrap_err();

        assert_matches!(error, LlmCallError::Permanent { .. });
        assert!(error.to_string().contains("missing tool_use content block"));
    }

    #[test]
    fn parses_context_overflow_error() {
        let response = Response {
            status: StatusCode::BAD_REQUEST,
            body: json!({
                "type": "error",
                "error": {
                    "type": "invalid_request_error",
                    "message": "prompt is too long"
                },
                "request_id": "req_test"
            }),
        };

        let error = provider().parse_response(response).unwrap_err();

        assert_matches!(error, LlmCallError::ContextOverflow { .. });
    }

    #[test]
    fn parses_context_overflow_stop_reason() {
        let response = Response {
            status: StatusCode::OK,
            body: json!({
                "content": [{
                    "type": "text",
                    "text": "truncated response"
                }],
                "stop_reason": "model_context_window_exceeded",
                "usage": {
                    "input_tokens": 100,
                    "output_tokens": 25
                }
            }),
        };

        let error = provider().parse_response(response).unwrap_err();

        assert_matches!(error, LlmCallError::ContextOverflow { .. });
    }

    #[test]
    fn does_not_treat_large_http_payload_as_context_overflow() {
        let response = Response {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            body: json!({
                "type": "error",
                "error": {
                    "type": "request_too_large",
                    "message": "Request too large"
                }
            }),
        };

        let error = provider().parse_response(response).unwrap_err();

        assert_matches!(error, LlmCallError::Permanent { .. });
    }

    #[test]
    fn parses_retryable_error_as_transient() {
        let response = Response {
            status: StatusCode::TOO_MANY_REQUESTS,
            body: json!({
                "error": {
                    "message": "rate limit exceeded"
                }
            }),
        };

        let error = provider().parse_response(response).unwrap_err();

        assert_matches!(error, LlmCallError::Transient { .. });
        assert!(std::error::Error::source(&error).is_some());
    }

    #[test]
    fn parses_non_retryable_error_as_permanent() {
        let response = Response {
            status: StatusCode::UNAUTHORIZED,
            body: json!({
                "error": {
                    "message": "invalid API key"
                }
            }),
        };

        let error = provider().parse_response(response).unwrap_err();

        assert_matches!(error, LlmCallError::Permanent { .. });
        assert!(error.to_string().contains("invalid API key"));
        assert!(std::error::Error::source(&error).is_some());
    }
}
