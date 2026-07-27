use std::fmt;

use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde_json::json;

use crate::secret::Secret;

use super::{
    ConversationTurn, LlmCallError, LlmCallResult, LlmProvider, LlmRequest, LlmResponse, RawUsage,
    Request, Response, ToolCall, ToolSpec,
};

const DEFAULT_BASE_URL: &str = "https://api.openai.com";

#[derive(Debug, Clone)]
pub struct OpenAiProvider {
    api_key: Secret,
    base_url: String,
}

impl OpenAiProvider {
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

impl LlmProvider for OpenAiProvider {
    fn build_request(
        &self,
        request: LlmRequest<'_>,
        is_last_request: bool,
    ) -> Result<Request, LlmCallError> {
        let mut authorization = format!("Bearer {}", self.api_key.expose_secret())
            .parse::<HeaderValue>()
            .map_err(|error| LlmCallError::Permanent {
                message: "failed to build OpenAI authorization header".to_string(),
                source: Box::new(error),
            })?;
        authorization.set_sensitive(true);

        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, authorization);

        Ok(Request {
            url: format!("{}/v1/responses", self.base_url),
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
    let mut input = Vec::new();
    for turn in request.conversation {
        match turn {
            ConversationTurn::System(content) => {
                input.push(json!({ "role": "system", "content": content }))
            }
            ConversationTurn::User(content) => {
                input.push(json!({ "role": "user", "content": content }))
            }
            ConversationTurn::AssistantToolCalls(tool_calls) => {
                if let Some(tool_call) = tool_calls.first() {
                    let reasoning_items = match provider_state(tool_call)? {
                        Some(provider_state) => {
                            let error =
                                invalid_provider_state(tool_call, "missing reasoning array");
                            provider_state
                                .get("reasoning")
                                .and_then(serde_json::Value::as_array)
                                .cloned()
                                .ok_or(error)?
                        }
                        None => Vec::new(),
                    };
                    input.extend(reasoning_items);
                }
                input.extend(
                    tool_calls
                        .iter()
                        .map(function_call)
                        .collect::<Result<Vec<_>, _>>()?,
                );
            }
            ConversationTurn::ToolResult { call_id, result } => input.push(json!({
                "type": "function_call_output",
                "call_id": call_id,
                "output": result.to_string(),
            })),
        }
    }

    let tools: Vec<_> = request.tools.iter().map(tool).collect();
    let mut body = json!({
        "model": request.model,
        "input": input,
        "tools": tools,
        "tool_choice": "required",
    });
    if is_last_request {
        body["parallel_tool_calls"] = json!(false);
    }

    Ok(body)
}

fn tool(spec: &ToolSpec) -> serde_json::Value {
    json!({
        "type": "function",
        "name": spec.name,
        "description": spec.description,
        "parameters": spec.parameters,
        "strict": false,
    })
}

fn function_call(tool_call: &ToolCall) -> Result<serde_json::Value, LlmCallError> {
    let Some(provider_state) = provider_state(tool_call)? else {
        return Ok(json!({
            "type": "function_call",
            "call_id": tool_call.id,
            "name": tool_call.name,
            "arguments": tool_call.arguments.to_string(),
        }));
    };

    let error = invalid_provider_state(tool_call, "missing function_call item");
    let function_call = provider_state.get("function_call").cloned().ok_or(error)?;
    if function_call
        .get("type")
        .and_then(serde_json::Value::as_str)
        != Some("function_call")
        || function_call
            .get("call_id")
            .and_then(serde_json::Value::as_str)
            != Some(tool_call.id.as_str())
    {
        return Err(invalid_provider_state(
            tool_call,
            "function_call item does not match tool call",
        ));
    }

    Ok(function_call)
}

fn provider_state(tool_call: &ToolCall) -> Result<Option<serde_json::Value>, LlmCallError> {
    tool_call
        .provider_state
        .as_deref()
        .map(|state| {
            serde_json::from_str(state).map_err(|error| LlmCallError::Permanent {
                message: format!(
                    "invalid provider_state JSON for tool call {}: {error}",
                    tool_call.id
                ),
                source: Box::new(error),
            })
        })
        .transpose()
}

fn invalid_provider_state(tool_call: &ToolCall, message: &str) -> LlmCallError {
    permanent_parse_error(format!(
        "invalid provider_state for tool call {}: {message}",
        tool_call.id
    ))
}

fn parse_success(body: &serde_json::Value) -> Result<LlmCallResult, LlmCallError> {
    let output = body
        .get("output")
        .and_then(serde_json::Value::as_array)
        .ok_or(permanent_parse_error("missing output array"))?;
    let usage = RawUsage {
        input_tokens: required_u64(body, "/usage/input_tokens")?,
        output_tokens: required_u64(body, "/usage/output_tokens")?,
    };
    let reasoning = output
        .iter()
        .filter(|item| item.get("type").and_then(serde_json::Value::as_str) == Some("reasoning"))
        .cloned()
        .collect::<Vec<_>>();
    let tool_calls = output
        .iter()
        .filter(|item| {
            item.get("type").and_then(serde_json::Value::as_str) == Some("function_call")
        })
        .map(|item| parse_tool_call(item, &reasoning))
        .collect::<Result<Vec<_>, _>>()?;
    if tool_calls.is_empty() {
        return Err(permanent_parse_error("missing function_call item"));
    }

    Ok(LlmCallResult {
        response: LlmResponse::ToolCalls(tool_calls),
        usage,
    })
}

fn parse_tool_call(
    value: &serde_json::Value,
    reasoning: &[serde_json::Value],
) -> Result<ToolCall, LlmCallError> {
    let id = required_string(value, "/call_id")?.to_string();
    let name = required_string(value, "/name")?.to_string();
    let arguments = required_string(value, "/arguments")?;
    let arguments = serde_json::from_str(arguments).map_err(|error| LlmCallError::Permanent {
        message: format!("failed to parse tool call arguments for {name}"),
        source: Box::new(error),
    })?;

    Ok(ToolCall {
        id,
        name,
        arguments,
        provider_state: Some(json!({ "reasoning": reasoning, "function_call": value }).to_string()),
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
        .unwrap_or("OpenAI request failed")
        .to_string();

    if is_context_overflow(status, body, &message) {
        return LlmCallError::ContextOverflow { message };
    }

    let source = Box::new(OpenAiStatusError {
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

    if body
        .pointer("/error/code")
        .and_then(serde_json::Value::as_str)
        == Some("context_length_exceeded")
    {
        return true;
    }

    let message = message.to_ascii_lowercase();
    let identifies_context = message.contains("context window")
        || message.contains("context length")
        || message.contains("prompt is too long");
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
        source: Box::new(OpenAiParseError(message.clone())),
        message,
    }
}

#[derive(Debug)]
struct OpenAiParseError(String);

impl fmt::Display for OpenAiParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for OpenAiParseError {}

#[derive(Debug)]
struct OpenAiStatusError {
    status: u16,
    message: String,
}

impl fmt::Display for OpenAiStatusError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "OpenAI returned HTTP {}: {}",
            self.status, self.message
        )
    }
}

impl std::error::Error for OpenAiStatusError {}

#[cfg(test)]
mod tests {
    use super::*;

    use std::assert_matches;

    use reqwest::StatusCode;

    fn provider() -> OpenAiProvider {
        OpenAiProvider::new(
            Secret::new("test-api-key".to_string()),
            Some("https://openai.example.test/"),
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
    fn builds_responses_request_with_auth_input_and_tools() {
        let tools = [
            ToolSpec {
                name: "commit_diff".to_string(),
                description: "Read a commit diff".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "hash": {
                            "type": "string"
                        },
                    },
                    "required": ["hash"],
                }),
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
                    model: "gpt-4.1",
                    conversation: &conversation,
                    tools: &tools,
                },
                false,
            )
            .unwrap();

        assert_eq!(request.url, "https://openai.example.test/v1/responses");
        assert_eq!(request.headers[AUTHORIZATION], "Bearer test-api-key");
        assert!(request.headers[AUTHORIZATION].is_sensitive());
        assert_eq!(request.body["model"], "gpt-4.1");
        assert_eq!(request.body["input"][0]["role"], "system");
        assert_eq!(request.body["input"][0]["content"], "You review code.");
        assert_eq!(request.body["input"][1]["role"], "user");
        assert_eq!(request.body["tools"].as_array().unwrap().len(), tools.len());
        assert_eq!(request.body["tools"][0]["name"], "commit_diff");
        assert_eq!(request.body["tools"][1]["name"], "test_tool");
        assert_eq!(request.body["tools"][0]["strict"], false);
        assert_eq!(request.body["tool_choice"], "required");
        assert!(request.body.get("response_format").is_none());
    }

    #[test]
    fn last_request_requires_one_tool_call() {
        let tools = [test_tool()];

        let request = provider()
            .build_request(
                LlmRequest {
                    model: "gpt-4.1",
                    conversation: &[],
                    tools: &tools,
                },
                true,
            )
            .unwrap();

        assert_eq!(request.body["tool_choice"], "required");
        assert_eq!(request.body["parallel_tool_calls"], false);
    }

    #[test]
    fn encodes_assistant_tool_calls_and_tool_results() {
        let conversation = [
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
                    model: "gpt-4.1",
                    conversation: &conversation,
                    tools: &[],
                },
                false,
            )
            .unwrap();

        assert_eq!(request.body["input"][0]["type"], "function_call");
        assert_eq!(request.body["input"][0]["call_id"], "call-1");
        assert_eq!(
            request.body["input"][0]["arguments"],
            "{\"hash\":\"abc1234\"}"
        );
        assert_eq!(request.body["input"][1]["type"], "function_call_output");
        assert_eq!(request.body["input"][1]["call_id"], "call-1");
        assert_eq!(request.body["input"][1]["output"], "{\"diff\":\"+hello\"}");
        assert!(request.body["tools"].as_array().unwrap().is_empty());
    }

    #[test]
    fn rejects_invalid_provider_state_json_when_replaying_tool_calls() {
        let conversation = [ConversationTurn::AssistantToolCalls(vec![ToolCall {
            id: "call-1".to_string(),
            name: "commit_diff".to_string(),
            arguments: json!({ "hash": "abc1234" }),
            provider_state: Some("not json".to_string()),
        }])];

        let error = provider()
            .build_request(
                LlmRequest {
                    model: "gpt-4.1",
                    conversation: &conversation,
                    tools: &[],
                },
                false,
            )
            .unwrap_err();

        assert_matches!(error, LlmCallError::Permanent { .. });
        assert!(
            error
                .to_string()
                .contains("invalid provider_state JSON for tool call call-1")
        );
    }

    #[test]
    fn rejects_malformed_provider_state_when_replaying_tool_calls() {
        let conversation = [ConversationTurn::AssistantToolCalls(vec![ToolCall {
            id: "call-1".to_string(),
            name: "commit_diff".to_string(),
            arguments: json!({ "hash": "abc1234" }),
            provider_state: Some(json!({ "function_call": {} }).to_string()),
        }])];

        let error = provider()
            .build_request(
                LlmRequest {
                    model: "gpt-4.1",
                    conversation: &conversation,
                    tools: &[],
                },
                false,
            )
            .unwrap_err();

        assert_matches!(error, LlmCallError::Permanent { .. });
        assert!(
            error
                .to_string()
                .contains("invalid provider_state for tool call call-1: missing reasoning array")
        );
    }

    #[test]
    fn request_debug_redacts_api_key() {
        let request = provider()
            .build_request(
                LlmRequest {
                    model: "gpt-4.1",
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
        let name = "PEER_TEST_MISSING_OPENAI_API_KEY_4B9D5E7C9A1F";

        let error = OpenAiProvider::from_env(name, None).unwrap_err();

        assert_matches!(error, LlmCallError::Permanent { .. });
        assert!(error.to_string().contains(name));
    }

    #[test]
    fn parses_tool_call_response() {
        let response = Response {
            status: StatusCode::OK,
            body: json!({
                "output": [{
                    "id": "rs-1",
                    "type": "reasoning",
                    "summary": []
                }, {
                    "id": "fc-1",
                    "call_id": "call-1",
                    "type": "function_call",
                    "name": "commit_diff",
                    "arguments": "{\"hash\":\"abc1234\"}"
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
        assert!(tool_calls[0].provider_state.is_some());
    }

    #[test]
    fn rejects_response_without_tool_calls() {
        let response = Response {
            status: StatusCode::OK,
            body: json!({
                "output": [{
                    "type": "message",
                    "role": "assistant",
                    "content": [{
                        "type": "output_text",
                        "text": "{\"summary\":\"content json\",\"findings\":[]}"
                    }]
                }],
                "usage": {
                    "input_tokens": 10,
                    "output_tokens": 5
                }
            }),
        };

        let error = provider().parse_response(response).unwrap_err();

        assert_matches!(error, LlmCallError::Permanent { .. });
        assert!(error.to_string().contains("missing function_call item"));
    }

    #[test]
    fn parses_context_overflow_error() {
        let response = Response {
            status: StatusCode::BAD_REQUEST,
            body: json!({
                "error": {
                    "message": "Your input is too large for this model.",
                    "type": "invalid_request_error",
                    "param": "input",
                    "code": "context_length_exceeded"
                }
            }),
        };

        let error = provider().parse_response(response).unwrap_err();

        assert_matches!(error, LlmCallError::ContextOverflow { .. });
    }

    #[test]
    fn does_not_treat_token_parameter_error_as_context_overflow() {
        let response = Response {
            status: StatusCode::BAD_REQUEST,
            body: json!({
                "error": {
                    "message": "Invalid value for max_completion_tokens: must be at least 1",
                    "type": "invalid_request_error",
                    "param": "max_completion_tokens",
                    "code": "invalid_value"
                }
            }),
        };

        let error = provider().parse_response(response).unwrap_err();

        assert_matches!(error, LlmCallError::Permanent { .. });
    }

    #[test]
    fn does_not_treat_large_http_payload_as_context_overflow() {
        let response = Response {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            body: json!({
                "error": {
                    "message": "Request payload is too large"
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
