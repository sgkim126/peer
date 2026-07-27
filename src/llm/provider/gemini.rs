use std::fmt;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::json;

use crate::secret::Secret;

use super::{
    ConversationTurn, LlmCallError, LlmCallResult, LlmProvider, LlmRequest, LlmResponse, RawUsage,
    Request, Response, ToolCall, ToolSpec,
};

const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com";
const API_KEY_HEADER: HeaderName = HeaderName::from_static("x-goog-api-key");

#[derive(Debug, Clone)]
pub struct GeminiProvider {
    api_key: Secret,
    base_url: String,
}

impl GeminiProvider {
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

impl LlmProvider for GeminiProvider {
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
                message: "failed to build Gemini API key header".to_string(),
                source: Box::new(error),
            })?;
        api_key.set_sensitive(true);

        let mut headers = HeaderMap::new();
        headers.insert(API_KEY_HEADER, api_key);

        let model_path = if request.model.starts_with("models/") {
            request.model.to_string()
        } else {
            format!("models/{}", request.model)
        };

        Ok(Request {
            url: format!("{}/v1beta/{}:generateContent", self.base_url, model_path),
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
    _is_last_request: bool,
) -> Result<serde_json::Value, LlmCallError> {
    let mut system = Vec::new();
    let mut contents = Vec::new();

    for turn in request.conversation {
        match turn {
            ConversationTurn::System(content) => system.push(content.as_str()),
            ConversationTurn::User(content) => contents.push(json!({
                "role": "user",
                "parts": [{
                    "text": content,
                }],
            })),
            ConversationTurn::AssistantToolCalls(tool_calls) => contents.push(json!({
                "role": "model",
                "parts": tool_calls.iter().map(function_call).collect::<Vec<_>>(),
            })),
            ConversationTurn::ToolResult { call_id, result } => {
                let call_name = call_id
                    .strip_prefix("gemini:")
                    .and_then(|value| value.split_once(':').map(|(_, name)| name))
                    .ok_or(permanent_parse_error(format!(
                        "invalid Gemini tool call id: {call_id}"
                    )))?;
                contents.push(json!({
                    "role": "user",
                    "parts": [{
                        "functionResponse": {
                            "name": call_name,
                            "response": {
                                "result": result
                            },
                        },
                    }],
                }))
            }
        }
    }

    let system_instruction = (!system.is_empty()).then(|| {
        json!({
            "parts": [{
                "text": system.join("\n\n"),
            }],
        })
    });
    let declarations: Vec<_> = request.tools.iter().map(tool).collect();

    let mut body = json!({
        "contents": contents,
        "tools": [{
            "functionDeclarations": declarations,
        }],
        "toolConfig": {
            "functionCallingConfig": {
                "mode": "ANY",
            },
        },
    });
    if let Some(system_instruction) = system_instruction {
        body["systemInstruction"] = system_instruction;
    }

    Ok(body)
}

fn function_call(tool_call: &ToolCall) -> serde_json::Value {
    let mut part = json!({
        "functionCall": {
            "name": tool_call.name,
            "args": tool_call.arguments,
        },
    });
    if let Some(signature) = &tool_call.provider_state {
        part["thoughtSignature"] = json!(signature);
    }
    part
}

fn tool(spec: &ToolSpec) -> serde_json::Value {
    json!({
        "name": spec.name,
        "description": spec.description,
        "parameters": spec.parameters,
    })
}

fn parse_success(body: &serde_json::Value) -> Result<LlmCallResult, LlmCallError> {
    let parts = body
        .pointer("/candidates/0/content/parts")
        .and_then(serde_json::Value::as_array)
        .ok_or(permanent_parse_error("missing candidates[0].content.parts"))?;
    let candidate_tokens = required_u64(body, "/usageMetadata/candidatesTokenCount")?;
    let thinking_tokens = body
        .pointer("/usageMetadata/thoughtsTokenCount")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default();
    let usage = RawUsage {
        input_tokens: required_u64(body, "/usageMetadata/promptTokenCount")?,
        output_tokens: candidate_tokens + thinking_tokens,
    };
    let tool_calls = parse_tool_calls(parts)?;

    Ok(LlmCallResult {
        response: LlmResponse::ToolCalls(tool_calls),
        usage,
    })
}

fn parse_tool_calls(parts: &[serde_json::Value]) -> Result<Vec<ToolCall>, LlmCallError> {
    let tool_calls = parts
        .iter()
        .filter(|part| part.get("functionCall").is_some())
        .enumerate()
        .map(|(index, part)| parse_tool_call(index, part))
        .collect::<Result<Vec<_>, _>>()?;

    if tool_calls.is_empty() {
        return Err(permanent_parse_error("missing functionCall part"));
    }

    Ok(tool_calls)
}

fn parse_tool_call(index: usize, part: &serde_json::Value) -> Result<ToolCall, LlmCallError> {
    let value = part
        .get("functionCall")
        .ok_or(permanent_parse_error("missing function call"))?;
    let name = required_string(value, "/name")?.to_string();
    let arguments = value
        .get("args")
        .cloned()
        .ok_or(permanent_parse_error("missing function call args"))?;

    let call_id = format!("gemini:{index}:{name}");
    Ok(ToolCall {
        id: call_id,
        name,
        arguments,
        provider_state: part
            .get("thoughtSignature")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
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
        .unwrap_or("Gemini request failed")
        .to_string();

    if is_context_overflow(status, body, &message) {
        return LlmCallError::ContextOverflow { message };
    }

    let source = Box::new(GeminiStatusError {
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

    let status = body
        .pointer("/error/status")
        .and_then(serde_json::Value::as_str);
    if !matches!(status, None | Some("INVALID_ARGUMENT")) {
        return false;
    }

    let message = message.to_ascii_lowercase();
    let identifies_input_tokens = message.contains("input token count");
    let exceeds_model_limit = message.contains("exceed")
        || message.contains("maximum number of tokens")
        || message.contains("only supports up to");

    identifies_input_tokens && exceeds_model_limit
}

fn is_transient_status(status: u16) -> bool {
    matches!(status, 408 | 409 | 425 | 429 | 500 | 502 | 503 | 504)
}

fn permanent_parse_error(message: impl Into<String>) -> LlmCallError {
    let message = message.into();
    LlmCallError::Permanent {
        source: Box::new(GeminiParseError(message.clone())),
        message,
    }
}

#[derive(Debug)]
struct GeminiParseError(String);

impl fmt::Display for GeminiParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for GeminiParseError {}

#[derive(Debug)]
struct GeminiStatusError {
    status: u16,
    message: String,
}

impl fmt::Display for GeminiStatusError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Gemini returned HTTP {}: {}",
            self.status, self.message
        )
    }
}

impl std::error::Error for GeminiStatusError {}

#[cfg(test)]
mod tests {
    use super::*;

    use std::assert_matches;

    use reqwest::StatusCode;

    fn provider() -> GeminiProvider {
        GeminiProvider::new(
            Secret::from_env_with("TEST_API_KEY", |_| Ok("test-api-key".to_string())).unwrap(),
            Some("https://gemini.example.test/"),
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
    fn builds_request_with_system_instruction_and_tools() {
        let parameters = json!({
            "type": "object",
            "properties": {
                "hash": {
                    "type": "string",
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
                    model: "gemini-2.5-pro",
                    conversation: &conversation,
                    tools: &tools,
                },
                false,
            )
            .unwrap();

        assert_eq!(
            request.url,
            "https://gemini.example.test/v1beta/models/gemini-2.5-pro:generateContent"
        );
        assert_eq!(request.headers[API_KEY_HEADER], "test-api-key");
        assert!(request.headers[API_KEY_HEADER].is_sensitive());
        assert_eq!(
            request.body["systemInstruction"]["parts"][0]["text"],
            "You review code."
        );
        assert_eq!(request.body["contents"][0]["role"], "user");
        assert_eq!(
            request.body["contents"][0]["parts"][0]["text"],
            "Check abc1234."
        );
        assert_eq!(
            request.body["tools"][0]["functionDeclarations"][0]["name"],
            "commit_diff"
        );
        assert_eq!(
            request.body["tools"][0]["functionDeclarations"][0]["parameters"],
            parameters
        );
        assert_eq!(
            request.body["tools"][0]["functionDeclarations"][1]["name"],
            "test_tool"
        );
        assert_eq!(
            request.body["toolConfig"]["functionCallingConfig"]["mode"],
            "ANY"
        );
        assert!(request.body.get("generationConfig").is_none());
    }

    #[test]
    fn last_request_requires_function_calling() {
        let tools = [test_tool()];

        let request = provider()
            .build_request(
                LlmRequest {
                    model: "gemini-2.5-pro",
                    conversation: &[],
                    tools: &tools,
                },
                true,
            )
            .unwrap();

        assert_eq!(
            request.body["toolConfig"]["functionCallingConfig"]["mode"],
            "ANY"
        );
    }

    #[test]
    fn encodes_tool_calls_signatures_and_results() {
        let conversation = [
            ConversationTurn::AssistantToolCalls(vec![ToolCall {
                id: "gemini:0:commit_diff".to_string(),
                name: "commit_diff".to_string(),
                arguments: json!({
                    "hash": "abc1234"
                }),
                provider_state: Some("opaque-signature".to_string()),
            }]),
            ConversationTurn::ToolResult {
                call_id: "gemini:0:commit_diff".to_string(),
                result: json!(["abc1234", "def5678"]),
            },
        ];

        let request = provider()
            .build_request(
                LlmRequest {
                    model: "models/gemini-2.5-pro",
                    conversation: &conversation,
                    tools: &[],
                },
                false,
            )
            .unwrap();

        assert_eq!(
            request.url,
            "https://gemini.example.test/v1beta/models/gemini-2.5-pro:generateContent"
        );
        assert_eq!(request.body["contents"][0]["role"], "model");
        assert_eq!(
            request.body["contents"][0]["parts"][0]["functionCall"]["name"],
            "commit_diff"
        );
        assert_eq!(
            request.body["contents"][0]["parts"][0]["thoughtSignature"],
            "opaque-signature"
        );
        assert_eq!(
            request.body["contents"][1]["parts"][0]["functionResponse"]["name"],
            "commit_diff"
        );
        assert_eq!(
            request.body["contents"][1]["parts"][0]["functionResponse"]["response"],
            json!({
                "result": ["abc1234", "def5678"]
            })
        );
    }

    #[test]
    fn rejects_non_gemini_tool_call_id() {
        let conversation = [ConversationTurn::ToolResult {
            call_id: "call-1".to_string(),
            result: json!({
                "diff": "+hello"
            }),
        }];

        let error = provider()
            .build_request(
                LlmRequest {
                    model: "gemini-2.5-pro",
                    conversation: &conversation,
                    tools: &[],
                },
                false,
            )
            .unwrap_err();

        assert_matches!(error, LlmCallError::Permanent { .. });
        assert!(error.to_string().contains("invalid Gemini tool call id"));
    }

    #[test]
    fn request_debug_redacts_api_key() {
        let request = provider()
            .build_request(
                LlmRequest {
                    model: "gemini-2.5-pro",
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
        let name = "PEER_TEST_MISSING_GEMINI_API_KEY_4B9D5E7C9A1F";

        let error = GeminiProvider::from_env(name, None).unwrap_err();

        assert_matches!(error, LlmCallError::Permanent { .. });
        assert!(error.to_string().contains(name));
    }

    #[test]
    fn parses_function_call_and_preserves_provider_state() {
        let response = Response {
            status: StatusCode::OK,
            body: json!({
                "candidates": [{
                    "content": {
                        "parts": [{
                            "functionCall": {
                                "name": "commit_diff",
                                "args": {
                                    "hash": "abc1234"
                                }
                            },
                            "thoughtSignature": "opaque-signature"
                        }]
                    }
                }],
                "usageMetadata": {
                    "promptTokenCount": 100,
                    "candidatesTokenCount": 25,
                    "thoughtsTokenCount": 15
                }
            }),
        };

        let result = provider().parse_response(response).unwrap();

        assert_eq!(result.usage.input_tokens, 100);
        assert_eq!(result.usage.output_tokens, 40);
        let LlmResponse::ToolCalls(tool_calls) = result.response;
        assert_eq!(tool_calls[0].id, "gemini:0:commit_diff");
        assert_eq!(tool_calls[0].name, "commit_diff");
        assert_eq!(
            tool_calls[0].arguments,
            json!({
                "hash": "abc1234"
            })
        );
        assert_eq!(
            tool_calls[0].provider_state.as_deref(),
            Some("opaque-signature")
        );
    }

    #[test]
    fn defaults_missing_thinking_token_count_to_zero() {
        let response = Response {
            status: StatusCode::OK,
            body: json!({
                "candidates": [{
                    "content": {
                        "parts": [{
                            "functionCall": {
                                "name": "commit_diff",
                                "args": {
                                    "hash": "abc1234"
                                }
                            }
                        }]
                    }
                }],
                "usageMetadata": {
                    "promptTokenCount": 100,
                    "candidatesTokenCount": 25
                }
            }),
        };

        let result = provider().parse_response(response).unwrap();

        assert_eq!(result.usage.output_tokens, 25);
    }

    #[test]
    fn rejects_response_without_function_calls() {
        let response = Response {
            status: StatusCode::OK,
            body: json!({
                "candidates": [{
                    "content": {
                        "parts": [{
                            "text": "{\"summary\":\"content json\",\"findings\":[]}"
                        }]
                    }
                }],
                "usageMetadata": {
                    "promptTokenCount": 10,
                    "candidatesTokenCount": 5
                }
            }),
        };

        let error = provider().parse_response(response).unwrap_err();

        assert_matches!(error, LlmCallError::Permanent { .. });
        assert!(error.to_string().contains("missing functionCall part"));
    }

    #[test]
    fn parses_context_overflow_error() {
        let response = Response {
            status: StatusCode::BAD_REQUEST,
            body: json!({
                "error": {
                    "code": 400,
                    "message": "The input token count (1048577) exceeds the maximum number of tokens allowed (1048576).",
                    "status": "INVALID_ARGUMENT"
                }
            }),
        };

        let error = provider().parse_response(response).unwrap_err();

        assert_matches!(error, LlmCallError::ContextOverflow { .. });
    }

    #[test]
    fn does_not_treat_output_token_parameter_error_as_context_overflow() {
        let response = Response {
            status: StatusCode::BAD_REQUEST,
            body: json!({
                "error": {
                    "code": 400,
                    "message": "maxOutputTokens must be greater than zero",
                    "status": "INVALID_ARGUMENT"
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
                    "code": 413,
                    "message": "Request payload is too large",
                    "status": "INVALID_ARGUMENT"
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
