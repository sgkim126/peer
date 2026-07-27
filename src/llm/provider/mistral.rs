use std::fmt;

use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde_json::json;

use crate::secret::Secret;

use super::{
    ConversationTurn, LlmCallError, LlmCallResult, LlmProvider, LlmRequest, LlmResponse, RawUsage,
    Request, Response, ToolCall, ToolSpec,
};

const DEFAULT_BASE_URL: &str = "https://api.mistral.ai";

#[derive(Debug, Clone)]
pub struct MistralProvider {
    api_key: Secret,
    base_url: String,
}

impl MistralProvider {
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

impl LlmProvider for MistralProvider {
    fn build_request(
        &self,
        request: LlmRequest<'_>,
        is_last_request: bool,
    ) -> Result<Request, LlmCallError> {
        let mut authorization = format!("Bearer {}", self.api_key.expose_secret())
            .parse::<HeaderValue>()
            .map_err(|error| LlmCallError::Permanent {
                message: "failed to build Mistral authorization header".to_string(),
                source: Box::new(error),
            })?;
        authorization.set_sensitive(true);

        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, authorization);

        Ok(Request {
            url: format!("{}/v1/chat/completions", self.base_url),
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
    let messages: Vec<_> = request
        .conversation
        .iter()
        .map(message)
        .collect::<Result<_, _>>()?;
    let tools: Vec<_> = request.tools.iter().map(tool).collect();

    let mut body = json!({
        "model": request.model,
        "messages": messages,
        "tools": tools,
        "tool_choice": "required",
    });
    if is_last_request {
        body["parallel_tool_calls"] = json!(false);
    }

    Ok(body)
}

fn message(turn: &ConversationTurn) -> Result<serde_json::Value, LlmCallError> {
    match turn {
        ConversationTurn::System(content) => Ok(json!({
            "role": "system",
            "content": content,
        })),
        ConversationTurn::User(content) => Ok(json!({
            "role": "user",
            "content": content,
        })),
        ConversationTurn::AssistantToolCalls(tool_calls) => Ok(json!({
            "role": "assistant",
            "content": null,
            "tool_calls": tool_calls
                .iter()
                .map(tool_call)
                .collect::<Result<Vec<_>, _>>()?,
        })),
        ConversationTurn::ToolResult { call_id, result } => Ok(json!({
            "role": "tool",
            "tool_call_id": call_id,
            "content": result.to_string(),
        })),
    }
}

fn tool(spec: &ToolSpec) -> serde_json::Value {
    json!({
        "type": "function",
        "function": {
            "name": spec.name,
            "description": spec.description,
            "parameters": spec.parameters,
        },
    })
}

fn tool_call(tool_call: &ToolCall) -> Result<serde_json::Value, LlmCallError> {
    Ok(json!({
        "id": tool_call.id,
        "type": "function",
        "function": {
            "name": tool_call.name,
            "arguments": serde_json::to_string(&tool_call.arguments).map_err(|error| {
                LlmCallError::Permanent {
                    message: format!("failed to encode tool call arguments: {error}"),
                    source: Box::new(error),
                }
            })?,
        },
    }))
}

fn parse_success(body: &serde_json::Value) -> Result<LlmCallResult, LlmCallError> {
    let message = body
        .pointer("/choices/0/message")
        .ok_or(permanent_parse_error("missing choices[0].message"))?;
    let usage = RawUsage {
        input_tokens: required_u64(body, "/usage/prompt_tokens")?,
        output_tokens: required_u64(body, "/usage/completion_tokens")?,
    };
    let tool_calls = parse_tool_calls(message)?;

    Ok(LlmCallResult {
        response: LlmResponse::ToolCalls(tool_calls),
        usage,
    })
}

fn parse_tool_calls(message: &serde_json::Value) -> Result<Vec<ToolCall>, LlmCallError> {
    let tool_calls = message
        .get("tool_calls")
        .and_then(serde_json::Value::as_array)
        .ok_or(permanent_parse_error(
            "missing choices[0].message.tool_calls",
        ))?
        .iter()
        .map(parse_tool_call)
        .collect::<Result<Vec<_>, _>>()?;

    if tool_calls.is_empty() {
        return Err(permanent_parse_error(
            "choices[0].message.tool_calls is empty",
        ));
    }

    Ok(tool_calls)
}

fn parse_tool_call(value: &serde_json::Value) -> Result<ToolCall, LlmCallError> {
    let id = required_string(value, "/id")?.to_string();
    let name = required_string(value, "/function/name")?.to_string();
    let arguments = required_string(value, "/function/arguments")?;
    let arguments = serde_json::from_str(arguments).map_err(|error| LlmCallError::Permanent {
        message: format!("failed to parse tool call arguments for {name}"),
        source: Box::new(error),
    })?;

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
        .pointer("/message")
        .or_else(|| body.pointer("/error/message"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("Mistral request failed")
        .to_string();

    if is_context_overflow(status, &message) {
        return LlmCallError::ContextOverflow { message };
    }

    let source = Box::new(MistralStatusError {
        status,
        message: message.clone(),
    });
    if is_transient_status(status) {
        LlmCallError::Transient { message, source }
    } else {
        LlmCallError::Permanent { message, source }
    }
}

fn is_context_overflow(status: u16, message: &str) -> bool {
    if status != 400 {
        return false;
    }

    let message = message.to_ascii_lowercase();
    let identifies_context = message.contains("context window")
        || message.contains("context length")
        || message.contains("input token count");
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
        source: Box::new(MistralParseError(message.clone())),
        message,
    }
}

#[derive(Debug)]
struct MistralParseError(String);

impl fmt::Display for MistralParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for MistralParseError {}

#[derive(Debug)]
struct MistralStatusError {
    status: u16,
    message: String,
}

impl fmt::Display for MistralStatusError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Mistral returned HTTP {}: {}",
            self.status, self.message
        )
    }
}

impl std::error::Error for MistralStatusError {}

#[cfg(test)]
mod tests {
    use super::*;

    use std::assert_matches;

    use reqwest::StatusCode;

    fn provider() -> MistralProvider {
        MistralProvider::new(
            Secret::new("test-api-key".to_string()),
            Some("https://mistral.example.test/"),
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
    fn builds_request_with_auth_messages_and_tools() {
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
                    model: "mistral-large-latest",
                    conversation: &conversation,
                    tools: &tools,
                },
                false,
            )
            .unwrap();

        assert_eq!(
            request.url,
            "https://mistral.example.test/v1/chat/completions"
        );
        assert_eq!(request.headers[AUTHORIZATION], "Bearer test-api-key");
        assert!(request.headers[AUTHORIZATION].is_sensitive());
        assert_eq!(request.body["model"], "mistral-large-latest");
        assert_eq!(request.body["messages"][0]["role"], "system");
        assert_eq!(request.body["messages"][0]["content"], "You review code.");
        assert_eq!(request.body["messages"][1]["role"], "user");
        assert_eq!(request.body["tools"].as_array().unwrap().len(), tools.len());
        assert_eq!(request.body["tools"][0]["function"]["name"], "commit_diff");
        assert_eq!(request.body["tools"][1]["function"]["name"], "test_tool");
        assert_eq!(request.body["tool_choice"], "required");
        assert!(request.body.get("response_format").is_none());
    }

    #[test]
    fn last_request_requires_one_tool_call() {
        let tools = [test_tool()];

        let request = provider()
            .build_request(
                LlmRequest {
                    model: "mistral-large-latest",
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
                    model: "mistral-large-latest",
                    conversation: &conversation,
                    tools: &[],
                },
                false,
            )
            .unwrap();

        assert_eq!(request.body["messages"][0]["role"], "assistant");
        assert_eq!(
            request.body["messages"][0]["tool_calls"][0]["function"]["arguments"],
            "{\"hash\":\"abc1234\"}"
        );
        assert_eq!(request.body["messages"][1]["role"], "tool");
        assert_eq!(request.body["messages"][1]["tool_call_id"], "call-1");
        assert_eq!(
            request.body["messages"][1]["content"],
            "{\"diff\":\"+hello\"}"
        );
        assert!(request.body["tools"].as_array().unwrap().is_empty());
    }

    #[test]
    fn request_debug_redacts_api_key() {
        let request = provider()
            .build_request(
                LlmRequest {
                    model: "mistral-large-latest",
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
        let name = "PEER_TEST_MISSING_MISTRAL_API_KEY_4B9D5E7C9A1F";

        let error = MistralProvider::from_env(name, None).unwrap_err();

        assert_matches!(error, LlmCallError::Permanent { .. });
        assert!(error.to_string().contains(name));
    }

    #[test]
    fn parses_tool_call_response() {
        let response = Response {
            status: StatusCode::OK,
            body: json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "tool_calls": [{
                            "id": "call-1",
                            "type": "function",
                            "function": {
                                "name": "commit_diff",
                                "arguments": "{\"hash\":\"abc1234\"}"
                            }
                        }]
                    }
                }],
                "usage": {
                    "prompt_tokens": 100,
                    "completion_tokens": 25
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
    fn rejects_response_without_tool_calls() {
        let response = Response {
            status: StatusCode::OK,
            body: json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "{\"summary\":\"content json\",\"findings\":[]}"
                    }
                }],
                "usage": {
                    "prompt_tokens": 10,
                    "completion_tokens": 5
                }
            }),
        };

        let error = provider().parse_response(response).unwrap_err();

        assert_matches!(error, LlmCallError::Permanent { .. });
        assert!(
            error
                .to_string()
                .contains("missing choices[0].message.tool_calls")
        );
    }

    #[test]
    fn parses_context_overflow_error() {
        let response = Response {
            status: StatusCode::BAD_REQUEST,
            body: json!({
                "message": "Request exceeds the model's maximum context length"
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
                "message": "max_tokens must be greater than or equal to 1"
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
                "message": "Request payload is too large"
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
