use serde_json::json;

use std::fmt;

use super::{
    ConversationTurn, LlmCallError, LlmCallResult, LlmOutputMode, LlmProvider, LlmRequest,
    LlmResponse, RawUsage, ToolCall, ToolSpec,
};
use crate::console::Console;
use crate::llm::provider::http::ProviderHttpClient;
use crate::llm::result::CheckOutput;
use crate::secret::Secret;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiRequestBuilder {
    api_key: Secret,
    base_url: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpenAiHttpRequest {
    pub url: String,
    pub bearer_token: Secret,
    pub body: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct OpenAiProvider {
    client: ProviderHttpClient,
    request_builder: OpenAiRequestBuilder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenAiResponseParser;

const DEFAULT_BASE_URL: &str = "https://api.openai.com";
// FIXME: make the timeout configurable
const DEFAULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

impl OpenAiProvider {
    pub fn from_env(
        api_key_env: &str,
        base_url: Option<&str>,
        console: Console,
    ) -> Result<Self, LlmCallError> {
        let request_builder = OpenAiRequestBuilder::from_env(api_key_env, base_url)?;
        let client = reqwest::Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .map_err(|error| LlmCallError::Permanent {
                message: "failed to build OpenAI HTTP client".to_string(),
                source: Box::new(error),
            })?;

        Ok(Self {
            client: ProviderHttpClient::new(client, console, "openai", "OpenAI"),
            request_builder,
        })
    }
}

impl LlmProvider for OpenAiProvider {
    async fn send(&self, request: LlmRequest<'_>) -> Result<LlmCallResult, LlmCallError> {
        let http = self.request_builder.build(request)?;
        let request = self
            .client
            .post(&http.url)
            .bearer_auth(http.bearer_token.expose_secret());
        let response = self.client.send_json(request, &http.body).await?;

        if response.status.is_success() {
            OpenAiResponseParser::parse_success(&response.body)
        } else {
            Err(OpenAiResponseParser::parse_error(
                response.status.as_u16(),
                &response.body,
            ))
        }
    }
}

impl OpenAiRequestBuilder {
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

    pub fn build(&self, request: LlmRequest<'_>) -> Result<OpenAiHttpRequest, LlmCallError> {
        Ok(OpenAiHttpRequest {
            url: format!("{}/v1/chat/completions", self.base_url),
            bearer_token: self.api_key.clone(),
            body: request_body(request)?,
        })
    }
}

impl OpenAiResponseParser {
    pub fn parse_success(body: &serde_json::Value) -> Result<LlmCallResult, LlmCallError> {
        let message = body
            .pointer("/choices/0/message")
            .ok_or_else(|| permanent_parse_error("missing choices[0].message"))?;
        let usage = parse_usage(body)?;
        let response = parse_response(message)?;

        Ok(LlmCallResult { response, usage })
    }

    pub fn parse_error(status: u16, body: &serde_json::Value) -> LlmCallError {
        let message = error_message(body);
        if is_context_overflow(status, &message) {
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
}

fn parse_usage(body: &serde_json::Value) -> Result<RawUsage, LlmCallError> {
    Ok(RawUsage {
        input_tokens: required_u32(body, "/usage/prompt_tokens")?,
        output_tokens: required_u32(body, "/usage/completion_tokens")?,
    })
}

fn parse_response(message: &serde_json::Value) -> Result<LlmResponse, LlmCallError> {
    if let Some(tool_calls) = message
        .get("tool_calls")
        .and_then(serde_json::Value::as_array)
    {
        let tool_calls = tool_calls
            .iter()
            .map(parse_tool_call)
            .collect::<Result<Vec<_>, _>>()?;

        if let Some(check_output) = check_output_response(&tool_calls)? {
            return Ok(LlmResponse::CheckOutput(check_output));
        }

        return Ok(LlmResponse::ToolCalls(tool_calls));
    }

    let content = message
        .get("content")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| permanent_parse_error("missing assistant content or tool_calls"))?;
    let value = serde_json::from_str(content).map_err(|error| LlmCallError::Permanent {
        message: "failed to parse assistant content as JSON".to_string(),
        source: Box::new(error),
    })?;
    let check_output = parse_check_output(value)?;

    Ok(LlmResponse::CheckOutput(check_output))
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
    })
}

fn check_output_response(tool_calls: &[ToolCall]) -> Result<Option<CheckOutput>, LlmCallError> {
    tool_calls
        .iter()
        .find(|tool_call| tool_call.name == STRUCTURED_OUTPUT_TOOL_NAME)
        .map(|tool_call| parse_check_output(tool_call.arguments.clone()))
        .transpose()
}

fn parse_check_output(value: serde_json::Value) -> Result<CheckOutput, LlmCallError> {
    serde_json::from_value(value).map_err(|error| LlmCallError::Permanent {
        message: "failed to parse check output".to_string(),
        source: Box::new(error),
    })
}

fn required_string<'a>(
    value: &'a serde_json::Value,
    pointer: &str,
) -> Result<&'a str, LlmCallError> {
    value
        .pointer(pointer)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| permanent_parse_error(format!("missing string at {pointer}")))
}

fn required_u32(value: &serde_json::Value, pointer: &str) -> Result<u32, LlmCallError> {
    let raw = value
        .pointer(pointer)
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| permanent_parse_error(format!("missing unsigned integer at {pointer}")))?;
    u32::try_from(raw).map_err(|error| LlmCallError::Permanent {
        message: format!("integer at {pointer} does not fit into u32"),
        source: Box::new(error),
    })
}

fn error_message(body: &serde_json::Value) -> String {
    body.pointer("/error/message")
        .or_else(|| body.pointer("/message"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("OpenAI request failed")
        .to_string()
}

fn is_context_overflow(status: u16, message: &str) -> bool {
    if status != 400 && status != 413 {
        return false;
    }

    let message = message.to_ascii_lowercase();
    message.contains("context")
        || message.contains("maximum")
        || message.contains("token")
        || message.contains("too large")
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
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for OpenAiParseError {}

#[derive(Debug)]
struct OpenAiStatusError {
    status: u16,
    message: String,
}

impl fmt::Display for OpenAiStatusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "OpenAI returned HTTP {}: {}", self.status, self.message)
    }
}

impl std::error::Error for OpenAiStatusError {}

fn request_body(request: LlmRequest<'_>) -> Result<serde_json::Value, LlmCallError> {
    Ok(match request.output_mode {
        LlmOutputMode::Check {
            tools: tool_specs,
            output_schema,
        } => json!({
            "model": request.model,
            "messages": messages(request.conversation)?,
            "tools": tools(tool_specs, output_schema),
            "tool_choice": "auto",
            "response_format": {
                "type": "json_object"
            },
        }),
    })
}

fn messages(conversation: &[ConversationTurn]) -> Result<Vec<serde_json::Value>, LlmCallError> {
    conversation.iter().map(message).collect()
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
            "tool_calls": tool_calls.iter().map(openai_tool_call).collect::<Result<Vec<_>, _>>()?,
        })),
        ConversationTurn::ToolResult { call_id, result } => Ok(json!({
            "role": "tool",
            "tool_call_id": call_id,
            "content": result.to_string(),
        })),
        ConversationTurn::AssistantCheckOutput(output) => Ok(json!({
            "role": "assistant",
            "content": serde_json::to_string(output).map_err(|error| LlmCallError::Permanent {
                message: "failed to encode assistant check output".to_string(),
                source: Box::new(error),
            })?,
        })),
    }
}

fn openai_tool_call(tool_call: &ToolCall) -> Result<serde_json::Value, LlmCallError> {
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

fn tools(tool_specs: &[ToolSpec], output_schema: &serde_json::Value) -> Vec<serde_json::Value> {
    tool_specs
        .iter()
        .map(tool)
        .chain(std::iter::once(structured_output_tool(output_schema)))
        .collect()
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

const STRUCTURED_OUTPUT_TOOL_NAME: &str = "submit_check_result";
fn structured_output_tool(output_schema: &serde_json::Value) -> serde_json::Value {
    json!({
        "type": "function",
        "function": {
            "name": STRUCTURED_OUTPUT_TOOL_NAME,
            "description": "Submit the final structured check result.",
            "parameters": output_schema,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output_schema() -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "summary": {
                    "type": "string"
                }
            },
            "required": ["summary"]
        })
    }

    #[test]
    fn builds_openai_request_body_with_messages_tools_and_structured_output_tool() {
        let builder = OpenAiRequestBuilder::new(
            Secret::new("test-api-key"),
            Some("https://openai.example.test/"),
        );
        let extract_tool = ToolSpec {
            name: "commit_diff".to_string(),
            description: "Read a commit diff".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "hash": {
                        "type": "string"
                    }
                },
                "required": ["hash"]
            }),
        };
        let schema = output_schema();
        let conversation = [
            ConversationTurn::System("You review code.".to_string()),
            ConversationTurn::User("Check abc1234.".to_string()),
        ];
        let request = LlmRequest {
            model: "gpt-4.1",
            conversation: &conversation,
            output_mode: LlmOutputMode::Check {
                tools: &[extract_tool],
                output_schema: &schema,
            },
        };

        let http = builder.build(request).unwrap();

        assert_eq!(http.url, "https://openai.example.test/v1/chat/completions");
        assert_eq!(http.bearer_token.expose_secret(), "test-api-key");
        assert_eq!(http.body["model"], "gpt-4.1");
        assert_eq!(http.body["messages"][0]["role"], "system");
        assert_eq!(http.body["messages"][0]["content"], "You review code.");
        assert_eq!(http.body["messages"][1]["role"], "user");
        assert_eq!(http.body["tools"][0]["function"]["name"], "commit_diff");
        assert_eq!(
            http.body["tools"][1]["function"]["name"],
            STRUCTURED_OUTPUT_TOOL_NAME
        );
        assert_eq!(http.body["tools"][1]["function"]["parameters"], schema);
        assert_eq!(http.body["tool_choice"], "auto");
        assert_eq!(
            http.body["response_format"],
            json!({ "type": "json_object" })
        );
    }

    #[test]
    fn encodes_assistant_tool_calls_and_tool_results() {
        let builder = OpenAiRequestBuilder::new(Secret::new("test-api-key"), None);
        let schema = output_schema();
        let conversation = [
            ConversationTurn::AssistantToolCalls(vec![ToolCall {
                id: "call-1".to_string(),
                name: "commit_diff".to_string(),
                arguments: json!({ "hash": "abc1234" }),
            }]),
            ConversationTurn::ToolResult {
                call_id: "call-1".to_string(),
                result: json!({ "diff": "+hello" }),
            },
        ];
        let request = LlmRequest {
            model: "gpt-4.1",
            conversation: &conversation,
            output_mode: LlmOutputMode::Check {
                tools: &[],
                output_schema: &schema,
            },
        };

        let http = builder.build(request).unwrap();

        assert_eq!(http.body["messages"][0]["role"], "assistant");
        assert_eq!(
            http.body["messages"][0]["tool_calls"][0]["function"]["arguments"],
            "{\"hash\":\"abc1234\"}"
        );
        assert_eq!(http.body["messages"][1]["role"], "tool");
        assert_eq!(http.body["messages"][1]["tool_call_id"], "call-1");
        assert_eq!(http.body["messages"][1]["content"], "{\"diff\":\"+hello\"}");
    }

    #[test]
    fn missing_api_key_is_permanent_error() {
        let name = "PEER_TEST_MISSING_OPENAI_API_KEY_4B9D5E7C9A1F";

        let error = OpenAiRequestBuilder::from_env(name, None).unwrap_err();

        assert!(matches!(error, LlmCallError::Permanent { .. }));
        assert!(error.to_string().contains(name));
    }

    #[test]
    fn provider_from_env_fails_when_api_key_is_missing() {
        let name = "PEER_TEST_MISSING_OPENAI_PROVIDER_API_KEY_92F4A1C8D3";

        let error = OpenAiProvider::from_env(name, None, Console::default()).unwrap_err();

        assert!(matches!(error, LlmCallError::Permanent { .. }));
        assert!(error.to_string().contains(name));
        assert!(std::error::Error::source(&error).is_some());
    }

    #[test]
    fn debug_redacts_api_key_and_bearer_token() {
        let builder = OpenAiRequestBuilder::new(Secret::new("test-api-key"), None);
        let schema = output_schema();
        let request = LlmRequest {
            model: "gpt-4.1",
            conversation: &[],
            output_mode: LlmOutputMode::Check {
                tools: &[],
                output_schema: &schema,
            },
        };

        let http = builder.build(request).unwrap();

        let builder_debug = format!("{builder:?}");
        let http_debug = format!("{http:?}");
        assert!(!builder_debug.contains("test-api-key"));
        assert!(!http_debug.contains("test-api-key"));
        assert!(builder_debug.contains("<******>"));
        assert!(http_debug.contains("<******>"));
    }

    #[test]
    fn parses_tool_call_response() {
        let body = json!({
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
        });

        let result = OpenAiResponseParser::parse_success(&body).unwrap();

        assert_eq!(result.usage.input_tokens, 100);
        assert_eq!(result.usage.output_tokens, 25);
        let LlmResponse::ToolCalls(tool_calls) = result.response else {
            panic!("expected tool calls");
        };
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "call-1");
        assert_eq!(tool_calls[0].name, "commit_diff");
        assert_eq!(tool_calls[0].arguments, json!({ "hash": "abc1234" }));
    }

    #[test]
    fn parses_structured_output_tool_response() {
        let body = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "call-structured",
                        "type": "function",
                        "function": {
                            "name": STRUCTURED_OUTPUT_TOOL_NAME,
                            "arguments": "{\"summary\":\"looks good\",\"findings\":[],\"confidence\":0.9}"
                        }
                    }]
                }
            }],
            "usage": {
                "prompt_tokens": 80,
                "completion_tokens": 40
            }
        });

        let result = OpenAiResponseParser::parse_success(&body).unwrap();

        assert_eq!(result.usage.input_tokens, 80);
        assert_eq!(result.usage.output_tokens, 40);
        let LlmResponse::CheckOutput(output) = result.response else {
            panic!("expected check output");
        };
        assert_eq!(output.summary, "looks good");
        assert_eq!(output.findings, vec![]);
        assert_eq!(output.confidence.as_f64(), 0.9);
    }

    #[test]
    fn parses_json_content_as_structured_response() {
        let body = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "{\"summary\":\"content json\",\"findings\":[],\"confidence\":0.8}"
                }
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5
            }
        });

        let result = OpenAiResponseParser::parse_success(&body).unwrap();

        let LlmResponse::CheckOutput(output) = result.response else {
            panic!("expected check output");
        };
        assert_eq!(output.summary, "content json");
        assert_eq!(output.findings, vec![]);
        assert_eq!(output.confidence.as_f64(), 0.8);
    }

    #[test]
    fn invalid_success_response_is_permanent_error() {
        let body = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "call-1",
                        "type": "function",
                        "function": {
                            "name": "commit_diff",
                            "arguments": "{"
                        }
                    }]
                }
            }],
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 25
            }
        });

        let error = OpenAiResponseParser::parse_success(&body).unwrap_err();

        assert!(matches!(error, LlmCallError::Permanent { .. }));
        assert!(error.to_string().contains("tool call arguments"));
        assert!(std::error::Error::source(&error).is_some());
    }

    #[test]
    fn parses_context_overflow_error() {
        let body = json!({
            "error": {
                "message": "maximum context length exceeded"
            }
        });

        let error = OpenAiResponseParser::parse_error(400, &body);

        assert!(matches!(error, LlmCallError::ContextOverflow { .. }));
    }

    #[test]
    fn parses_retryable_error_as_transient() {
        let body = json!({
            "error": {
                "message": "rate limit exceeded"
            }
        });

        let error = OpenAiResponseParser::parse_error(429, &body);

        assert!(matches!(error, LlmCallError::Transient { .. }));
        assert!(std::error::Error::source(&error).is_some());
    }

    #[test]
    fn parses_non_retryable_error_as_permanent() {
        let body = json!({
            "error": {
                "message": "invalid API key"
            }
        });

        let error = OpenAiResponseParser::parse_error(401, &body);

        assert!(matches!(error, LlmCallError::Permanent { .. }));
        assert!(error.to_string().contains("invalid API key"));
        assert!(std::error::Error::source(&error).is_some());
    }
}
