use serde_json::json;

use std::fmt;

use super::{
    ConversationTurn, LlmCallError, LlmCallResult, LlmProvider, LlmRequest, LlmResponse, RawUsage,
    ToolCall, ToolSpec,
};
use crate::console::Console;
use crate::llm::provider::debug::{format_headers_debug, format_json_debug};
use crate::llm::result::CheckOutput;
use crate::secret::Secret;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnthropicRequestBuilder {
    api_key: Secret,
    base_url: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AnthropicHttpRequest {
    pub url: String,
    pub api_key: Secret,
    pub body: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct AnthropicProvider {
    client: reqwest::Client,
    request_builder: AnthropicRequestBuilder,
    console: Console,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnthropicResponseParser;

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u32 = 4096;
// FIXME: make the timeout configurable
const DEFAULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

impl AnthropicProvider {
    pub fn from_env(
        api_key_env: &str,
        base_url: Option<&str>,
        console: Console,
    ) -> Result<Self, LlmCallError> {
        let request_builder = AnthropicRequestBuilder::from_env(api_key_env, base_url)?;
        let client = reqwest::Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .map_err(|error| LlmCallError::Permanent {
                message: "failed to build Anthropic HTTP client".to_string(),
                source: Box::new(error),
            })?;

        Ok(Self {
            client,
            request_builder,
            console,
        })
    }
}

impl LlmProvider for AnthropicProvider {
    async fn send(&self, request: LlmRequest<'_>) -> Result<LlmCallResult, LlmCallError> {
        let http = self.request_builder.build(request)?;
        self.console
            .debug(format_json_debug("anthropic request", &http.body));
        let request = self
            .client
            .post(&http.url)
            .header("x-api-key", http.api_key.expose_secret())
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&http.body)
            .build()
            .map_err(|error| LlmCallError::Permanent {
                message: "failed to build Anthropic HTTP request".to_string(),
                source: Box::new(error),
            })?;
        self.console.debug(format_headers_debug(
            "anthropic request headers",
            request.headers(),
        ));
        let response = self
            .client
            .execute(request)
            .await
            .map_err(map_transport_error)?;

        let status = response.status();
        self.console
            .debug(format!("anthropic response status={}", status.as_u16()));
        self.console.debug(format_headers_debug(
            "anthropic response headers",
            response.headers(),
        ));
        let body_text = response
            .text()
            .await
            .map_err(|error| LlmCallError::Permanent {
                message: "failed to read Anthropic response body".to_string(),
                source: Box::new(error),
            })?;
        self.console
            .debug(format!("anthropic response body\n{body_text}"));
        let body = serde_json::from_str::<serde_json::Value>(&body_text).map_err(|error| {
            LlmCallError::Permanent {
                message: "failed to parse Anthropic response JSON".to_string(),
                source: Box::new(error),
            }
        })?;

        if status.is_success() {
            AnthropicResponseParser::parse_success(&body)
        } else {
            Err(AnthropicResponseParser::parse_error(status.as_u16(), &body))
        }
    }
}

fn map_transport_error(error: reqwest::Error) -> LlmCallError {
    let message = error.to_string();
    if error.is_timeout() || error.is_connect() {
        LlmCallError::Transient {
            message,
            source: Box::new(error),
        }
    } else {
        LlmCallError::Permanent {
            message,
            source: Box::new(error),
        }
    }
}

impl AnthropicRequestBuilder {
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

    pub fn build(&self, request: LlmRequest<'_>) -> Result<AnthropicHttpRequest, LlmCallError> {
        let (system, messages) = messages(request.conversation)?;

        Ok(AnthropicHttpRequest {
            url: format!("{}/v1/messages", self.base_url),
            api_key: self.api_key.clone(),
            body: json!({
                "model": request.model,
                "max_tokens": DEFAULT_MAX_TOKENS,
                "system": system,
                "messages": messages,
                "tools": tools(request.tools, request.output_schema),
                "tool_choice": { "type": "auto" },
            }),
        })
    }
}

impl AnthropicResponseParser {
    pub fn parse_success(body: &serde_json::Value) -> Result<LlmCallResult, LlmCallError> {
        let content = body
            .get("content")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| permanent_parse_error("missing content array"))?;
        let usage = parse_usage(body)?;
        let response = parse_response(content)?;

        Ok(LlmCallResult { response, usage })
    }

    pub fn parse_error(status: u16, body: &serde_json::Value) -> LlmCallError {
        let message = error_message(body);
        if is_context_overflow(status, &message) {
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
}

fn parse_usage(body: &serde_json::Value) -> Result<RawUsage, LlmCallError> {
    Ok(RawUsage {
        input_tokens: required_u32(body, "/usage/input_tokens")?,
        output_tokens: required_u32(body, "/usage/output_tokens")?,
    })
}

fn parse_response(content: &[serde_json::Value]) -> Result<LlmResponse, LlmCallError> {
    let tool_calls = content
        .iter()
        .filter(|block| block.get("type").and_then(serde_json::Value::as_str) == Some("tool_use"))
        .map(parse_tool_call)
        .collect::<Result<Vec<_>, _>>()?;

    if !tool_calls.is_empty() {
        if let Some(check_output) = check_output_response(&tool_calls)? {
            return Ok(LlmResponse::CheckOutput(check_output));
        }

        return Ok(LlmResponse::ToolCalls(tool_calls));
    }

    let text = content
        .iter()
        .find(|block| block.get("type").and_then(serde_json::Value::as_str) == Some("text"))
        .and_then(|block| block.get("text"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| permanent_parse_error("missing text or tool_use content"))?;
    let value = serde_json::from_str(text).map_err(|error| LlmCallError::Permanent {
        message: "failed to parse assistant text as JSON".to_string(),
        source: Box::new(error),
    })?;

    Ok(LlmResponse::CheckOutput(parse_check_output(value)?))
}

fn parse_tool_call(value: &serde_json::Value) -> Result<ToolCall, LlmCallError> {
    let id = required_string(value, "/id")?.to_string();
    let name = required_string(value, "/name")?.to_string();
    let arguments = value
        .get("input")
        .cloned()
        .ok_or_else(|| permanent_parse_error("missing tool input"))?;

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
        .unwrap_or("Anthropic request failed")
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
        source: Box::new(AnthropicParseError(message.clone())),
        message,
    }
}

#[derive(Debug)]
struct AnthropicParseError(String);

impl fmt::Display for AnthropicParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for AnthropicParseError {}

#[derive(Debug)]
struct AnthropicStatusError {
    status: u16,
    message: String,
}

impl fmt::Display for AnthropicStatusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Anthropic returned HTTP {}: {}",
            self.status, self.message
        )
    }
}

impl std::error::Error for AnthropicStatusError {}

fn messages(
    conversation: &[ConversationTurn],
) -> Result<(String, Vec<serde_json::Value>), LlmCallError> {
    let mut system = Vec::new();
    let mut messages = Vec::new();

    for turn in conversation {
        match turn {
            ConversationTurn::System(content) => system.push(content.clone()),
            _ => messages.push(message(turn)?),
        }
    }

    Ok((system.join("\n\n"), messages))
}

fn message(turn: &ConversationTurn) -> Result<serde_json::Value, LlmCallError> {
    match turn {
        ConversationTurn::System(_) => unreachable!("system turns are handled separately"),
        ConversationTurn::User(content) => Ok(json!({
            "role": "user",
            "content": content,
        })),
        ConversationTurn::AssistantToolCalls(tool_calls) => Ok(json!({
            "role": "assistant",
            "content": tool_calls.iter().map(anthropic_tool_use).collect::<Vec<_>>(),
        })),
        ConversationTurn::ToolResult { call_id, result } => Ok(json!({
            "role": "user",
            "content": [{
                "type": "tool_result",
                "tool_use_id": call_id,
                "content": result.to_string(),
            }],
        })),
        ConversationTurn::AssistantCheckOutput(output) => Ok(json!({
            "role": "assistant",
            "content": [{
                "type": "text",
                "text": serde_json::to_string(output).map_err(|error| LlmCallError::Permanent {
                    message: "failed to encode assistant check output".to_string(),
                    source: Box::new(error),
                })?,
            }],
        })),
    }
}

fn anthropic_tool_use(tool_call: &ToolCall) -> serde_json::Value {
    json!({
        "type": "tool_use",
        "id": tool_call.id,
        "name": tool_call.name,
        "input": tool_call.arguments,
    })
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
        "name": spec.name,
        "description": spec.description,
        "input_schema": spec.parameters,
    })
}

const STRUCTURED_OUTPUT_TOOL_NAME: &str = "submit_check_result";
fn structured_output_tool(output_schema: &serde_json::Value) -> serde_json::Value {
    json!({
        "name": STRUCTURED_OUTPUT_TOOL_NAME,
        "description": "Submit the final structured check result.",
        "input_schema": output_schema,
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
    fn builds_anthropic_request_body_with_messages_tools_and_structured_output_tool() {
        let builder = AnthropicRequestBuilder::new(
            Secret::new("test-api-key"),
            Some("https://anthropic.example.test/"),
        );
        let extract_tool_parameters = json!({
            "type": "object",
            "properties": {
                "hash": {
                    "type": "string"
                }
            },
            "required": ["hash"]
        });
        let extract_tool = ToolSpec {
            name: "commit_diff".to_string(),
            description: "Read a commit diff".to_string(),
            parameters: extract_tool_parameters.clone(),
        };
        let schema = output_schema();
        let conversation = [
            ConversationTurn::System("You review code.".to_string()),
            ConversationTurn::User("Check abc1234.".to_string()),
        ];
        let request = LlmRequest {
            model: "claude-sonnet-4-5",
            conversation: &conversation,
            tools: &[extract_tool],
            output_schema: &schema,
        };

        let http = builder.build(request).unwrap();

        assert_eq!(http.url, "https://anthropic.example.test/v1/messages");
        assert_eq!(http.api_key.expose_secret(), "test-api-key");
        assert_eq!(http.body["model"], "claude-sonnet-4-5");
        assert_eq!(http.body["max_tokens"], DEFAULT_MAX_TOKENS);
        assert_eq!(http.body["system"], "You review code.");
        assert_eq!(http.body["messages"][0]["role"], "user");
        assert_eq!(http.body["messages"][0]["content"], "Check abc1234.");
        assert_eq!(http.body["tools"][0]["name"], "commit_diff");
        assert_eq!(
            http.body["tools"][0]["input_schema"],
            extract_tool_parameters
        );
        assert_eq!(http.body["tools"][1]["name"], STRUCTURED_OUTPUT_TOOL_NAME);
        assert_eq!(http.body["tools"][1]["input_schema"], schema);
        assert_eq!(http.body["tool_choice"], json!({ "type": "auto" }));
    }

    #[test]
    fn encodes_assistant_tool_calls_and_tool_results() {
        let builder = AnthropicRequestBuilder::new(Secret::new("test-api-key"), None);
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
            model: "claude-sonnet-4-5",
            conversation: &conversation,
            tools: &[],
            output_schema: &schema,
        };

        let http = builder.build(request).unwrap();

        assert_eq!(http.body["messages"][0]["role"], "assistant");
        assert_eq!(http.body["messages"][0]["content"][0]["type"], "tool_use");
        assert_eq!(
            http.body["messages"][0]["content"][0]["input"]["hash"],
            "abc1234"
        );
        assert_eq!(http.body["messages"][1]["role"], "user");
        assert_eq!(
            http.body["messages"][1]["content"][0]["type"],
            "tool_result"
        );
        assert_eq!(
            http.body["messages"][1]["content"][0]["tool_use_id"],
            "call-1"
        );
        assert_eq!(
            http.body["messages"][1]["content"][0]["content"],
            "{\"diff\":\"+hello\"}"
        );
    }

    #[test]
    fn missing_api_key_is_permanent_error() {
        let name = "PEER_TEST_MISSING_ANTHROPIC_API_KEY_4B9D5E7C9A1F";

        let error = AnthropicRequestBuilder::from_env(name, None).unwrap_err();

        assert!(matches!(error, LlmCallError::Permanent { .. }));
        assert!(error.to_string().contains(name));
    }

    #[test]
    fn provider_from_env_fails_when_api_key_is_missing() {
        let name = "PEER_TEST_MISSING_ANTHROPIC_PROVIDER_API_KEY_92F4A1C8D3";

        let error = AnthropicProvider::from_env(name, None, Console::default()).unwrap_err();

        assert!(matches!(error, LlmCallError::Permanent { .. }));
        assert!(error.to_string().contains(name));
        assert!(std::error::Error::source(&error).is_some());
    }

    #[test]
    fn debug_redacts_api_key() {
        let builder = AnthropicRequestBuilder::new(Secret::new("test-api-key"), None);
        let schema = output_schema();
        let request = LlmRequest {
            model: "claude-sonnet-4-5",
            conversation: &[],
            tools: &[],
            output_schema: &schema,
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
    fn parses_tool_use_response() {
        let body = json!({
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
        });

        let result = AnthropicResponseParser::parse_success(&body).unwrap();

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
            "content": [{
                "type": "tool_use",
                "id": "call-structured",
                "name": STRUCTURED_OUTPUT_TOOL_NAME,
                "input": {
                    "summary": "looks good",
                    "findings": [],
                    "confidence": 0.9
                }
            }],
            "usage": {
                "input_tokens": 80,
                "output_tokens": 40
            }
        });

        let result = AnthropicResponseParser::parse_success(&body).unwrap();

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
    fn parses_json_text_as_structured_response() {
        let body = json!({
            "content": [{
                "type": "text",
                "text": "{\"summary\":\"content json\",\"findings\":[],\"confidence\":0.8}"
            }],
            "usage": {
                "input_tokens": 10,
                "output_tokens": 5
            }
        });

        let result = AnthropicResponseParser::parse_success(&body).unwrap();

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
            "content": [{
                "type": "text",
                "text": "{"
            }],
            "usage": {
                "input_tokens": 100,
                "output_tokens": 25
            }
        });

        let error = AnthropicResponseParser::parse_success(&body).unwrap_err();

        assert!(matches!(error, LlmCallError::Permanent { .. }));
        assert!(error.to_string().contains("assistant text"));
        assert!(std::error::Error::source(&error).is_some());
    }

    #[test]
    fn parses_context_overflow_error() {
        let body = json!({
            "error": {
                "message": "maximum context length exceeded"
            }
        });

        let error = AnthropicResponseParser::parse_error(400, &body);

        assert!(matches!(error, LlmCallError::ContextOverflow { .. }));
    }

    #[test]
    fn parses_retryable_error_as_transient() {
        let body = json!({
            "error": {
                "message": "rate limit exceeded"
            }
        });

        let error = AnthropicResponseParser::parse_error(429, &body);

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

        let error = AnthropicResponseParser::parse_error(401, &body);

        assert!(matches!(error, LlmCallError::Permanent { .. }));
        assert!(error.to_string().contains("invalid API key"));
        assert!(std::error::Error::source(&error).is_some());
    }
}
