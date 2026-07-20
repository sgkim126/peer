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
        let output_mode = request.output_mode;
        let http = self.request_builder.build(request)?;
        let request = self
            .client
            .post(&http.url)
            .bearer_auth(http.bearer_token.expose_secret());
        let response = self.client.send_json(request, &http.body).await?;

        if response.status.is_success() {
            OpenAiResponseParser::parse_success(&response.body, output_mode)
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
            url: format!("{}/v1/responses", self.base_url),
            bearer_token: self.api_key.clone(),
            body: request_body(request)?,
        })
    }
}

impl OpenAiResponseParser {
    pub fn parse_success(
        body: &serde_json::Value,
        output_mode: LlmOutputMode<'_>,
    ) -> Result<LlmCallResult, LlmCallError> {
        let output = body
            .get("output")
            .and_then(serde_json::Value::as_array)
            .ok_or(permanent_parse_error("missing output array"))?;
        let usage = parse_usage(body)?;
        let response = parse_response(output, output_mode)?;

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
        input_tokens: required_u32(body, "/usage/input_tokens")?,
        output_tokens: required_u32(body, "/usage/output_tokens")?,
    })
}

fn parse_response(
    output: &[serde_json::Value],
    output_mode: LlmOutputMode<'_>,
) -> Result<LlmResponse, LlmCallError> {
    match output_mode {
        LlmOutputMode::Check { .. } => parse_check_response(output),
        LlmOutputMode::Text => parse_text_response(output),
    }
}

fn parse_check_response(output: &[serde_json::Value]) -> Result<LlmResponse, LlmCallError> {
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

    if !tool_calls.is_empty() {
        if let Some(check_output) = check_output_response(&tool_calls)? {
            return Ok(LlmResponse::CheckOutput(check_output));
        }

        return Ok(LlmResponse::ToolCalls(tool_calls));
    }

    let text = output_text(output, "missing output_text or function_call item")?;
    let value = serde_json::from_str(&text).map_err(|error| LlmCallError::Permanent {
        message: "failed to parse assistant content as JSON".to_string(),
        source: Box::new(error),
    })?;
    let check_output = parse_check_output(value)?;

    Ok(LlmResponse::CheckOutput(check_output))
}

fn parse_text_response(output: &[serde_json::Value]) -> Result<LlmResponse, LlmCallError> {
    Ok(LlmResponse::Text(output_text(
        output,
        "missing output_text item",
    )?))
}

fn output_text(
    output: &[serde_json::Value],
    missing_message: &str,
) -> Result<String, LlmCallError> {
    let text = output
        .iter()
        .filter(|item| item.get("type").and_then(serde_json::Value::as_str) == Some("message"))
        .filter_map(|item| item.get("content").and_then(serde_json::Value::as_array))
        .flatten()
        .filter(|content| {
            content.get("type").and_then(serde_json::Value::as_str) == Some("output_text")
        })
        .filter_map(|content| content.get("text").and_then(serde_json::Value::as_str))
        .collect::<String>();
    if text.is_empty() {
        Err(permanent_parse_error(missing_message))
    } else {
        Ok(text)
    }
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
        thought_signature: Some(
            json!({
                "reasoning": reasoning,
                "function_call": value,
            })
            .to_string(),
        ),
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
    let body = match request.output_mode {
        LlmOutputMode::Check {
            tools: tool_specs,
            output_schema,
        } => {
            let is_last_request = is_last_request(tool_specs);
            let mut body = json!({
            "model": request.model,
            "input": input_items(request.conversation)?,
            "tools": tools(tool_specs, output_schema),
            "tool_choice": if is_last_request { "required" } else { "auto" },
            "text": {
                "format": {
                    "type": "json_object"
                }
            },
            });
            if is_last_request {
                body["parallel_tool_calls"] = json!(false);
            }
            body
        }
        LlmOutputMode::Text => json!({
            "model": request.model,
            "input": input_items(request.conversation)?,
        }),
    };
    Ok(body)
}

fn is_last_request(tool_specs: &[ToolSpec]) -> bool {
    tool_specs.len() == 1 && tool_specs[0].name == "request_user_info"
}

fn input_items(conversation: &[ConversationTurn]) -> Result<Vec<serde_json::Value>, LlmCallError> {
    let mut items = Vec::new();
    for turn in conversation {
        match turn {
            ConversationTurn::System(content) => items.push(json!({
                "role": "system",
                "content": content,
            })),
            ConversationTurn::User(content) => items.push(json!({
                "role": "user",
                "content": content,
            })),
            ConversationTurn::AssistantToolCalls(tool_calls) => {
                if let Some(tool_call) = tool_calls.first() {
                    items.extend(openai_reasoning_items(tool_call));
                }
                items.extend(tool_calls.iter().map(openai_function_call));
            }
            ConversationTurn::ToolResult { call_id, result } => items.push(json!({
                "type": "function_call_output",
                "call_id": call_id,
                "output": result.to_string(),
            })),
        }
    }
    Ok(items)
}

fn openai_reasoning_items(tool_call: &ToolCall) -> Vec<serde_json::Value> {
    tool_call
        .thought_signature
        .as_deref()
        .and_then(|state| serde_json::from_str::<serde_json::Value>(state).ok())
        .and_then(|state| {
            state
                .get("reasoning")
                .and_then(serde_json::Value::as_array)
                .cloned()
        })
        .unwrap_or_default()
}

fn openai_function_call(tool_call: &ToolCall) -> serde_json::Value {
    if let Some(function_call) = tool_call
        .thought_signature
        .as_deref()
        .and_then(|state| serde_json::from_str::<serde_json::Value>(state).ok())
        .and_then(|state| state.get("function_call").cloned())
        .filter(|item| {
            item.get("type").and_then(serde_json::Value::as_str) == Some("function_call")
                && item.get("call_id").and_then(serde_json::Value::as_str)
                    == Some(tool_call.id.as_str())
        })
    {
        return function_call;
    }

    json!({
        "type": "function_call",
        "call_id": tool_call.id,
        "name": tool_call.name,
        "arguments": tool_call.arguments.to_string(),
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
        "type": "function",
        "name": spec.name,
        "description": spec.description,
        "parameters": spec.parameters,
        "strict": false,
    })
}

const STRUCTURED_OUTPUT_TOOL_NAME: &str = "submit_check_result";
fn structured_output_tool(output_schema: &serde_json::Value) -> serde_json::Value {
    json!({
        "type": "function",
        "name": STRUCTURED_OUTPUT_TOOL_NAME,
        "description": "Submit the final structured check result.",
        "parameters": output_schema,
        "strict": false,
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

    fn parse_success_check(body: &serde_json::Value) -> Result<LlmCallResult, LlmCallError> {
        let schema = output_schema();
        OpenAiResponseParser::parse_success(
            body,
            LlmOutputMode::Check {
                tools: &[],
                output_schema: &schema,
            },
        )
    }

    fn parse_success_text(body: &serde_json::Value) -> Result<LlmCallResult, LlmCallError> {
        OpenAiResponseParser::parse_success(body, LlmOutputMode::Text)
    }

    #[test]
    fn builds_responses_request_with_input_tools_and_structured_output_tool() {
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

        assert_eq!(http.url, "https://openai.example.test/v1/responses");
        assert_eq!(http.bearer_token.expose_secret(), "test-api-key");
        assert_eq!(http.body["model"], "gpt-4.1");
        assert_eq!(http.body["input"][0]["role"], "system");
        assert_eq!(http.body["input"][0]["content"], "You review code.");
        assert_eq!(http.body["input"][1]["role"], "user");
        assert_eq!(http.body["tools"][0]["name"], "commit_diff");
        assert_eq!(http.body["tools"][1]["name"], STRUCTURED_OUTPUT_TOOL_NAME);
        assert_eq!(http.body["tools"][1]["parameters"], schema);
        assert_eq!(http.body["tools"][0]["strict"], false);
        assert_eq!(http.body["tool_choice"], "auto");
        assert_eq!(
            http.body["text"]["format"],
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
                thought_signature: None,
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

        assert_eq!(http.body["input"][0]["type"], "function_call");
        assert_eq!(http.body["input"][0]["call_id"], "call-1");
        assert_eq!(http.body["input"][0]["arguments"], "{\"hash\":\"abc1234\"}");
        assert_eq!(http.body["input"][1]["type"], "function_call_output");
        assert_eq!(http.body["input"][1]["call_id"], "call-1");
        assert_eq!(http.body["input"][1]["output"], "{\"diff\":\"+hello\"}");
    }

    #[test]
    fn builds_text_request_body_without_tools_or_text_format() {
        let builder = OpenAiRequestBuilder::new(Secret::new("test-api-key"), None);
        let conversation = [ConversationTurn::User("Summarize this PR.".to_string())];
        let request = LlmRequest {
            model: "gpt-4.1",
            conversation: &conversation,
            output_mode: LlmOutputMode::Text,
        };

        let http = builder.build(request).unwrap();

        assert_eq!(http.body["model"], "gpt-4.1");
        assert_eq!(http.body["input"][0]["role"], "user");
        assert_eq!(http.body["input"][0]["content"], "Summarize this PR.");
        assert!(http.body.get("tools").is_none());
        assert!(http.body.get("tool_choice").is_none());
        assert!(http.body.get("text").is_none());
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
            "output": [
                {
                    "id": "rs-1",
                    "type": "reasoning",
                    "summary": []
                },
                {
                    "id": "fc-1",
                    "call_id": "call-1",
                    "type": "function_call",
                    "name": "commit_diff",
                    "arguments": "{\"hash\":\"abc1234\"}"
                }
            ],
            "usage": {
                "input_tokens": 100,
                "output_tokens": 25
            }
        });

        let result = parse_success_check(&body).unwrap();

        assert_eq!(result.usage.input_tokens, 100);
        assert_eq!(result.usage.output_tokens, 25);
        let LlmResponse::ToolCalls(tool_calls) = result.response else {
            panic!("expected tool calls");
        };
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "call-1");
        assert_eq!(tool_calls[0].name, "commit_diff");
        assert_eq!(tool_calls[0].arguments, json!({ "hash": "abc1234" }));

        let schema = output_schema();
        let conversation = [
            ConversationTurn::AssistantToolCalls(tool_calls),
            ConversationTurn::ToolResult {
                call_id: "call-1".to_string(),
                result: json!({ "diff": "+hello" }),
            },
        ];
        let request = LlmRequest {
            model: "gpt-5.6-luna",
            conversation: &conversation,
            output_mode: LlmOutputMode::Check {
                tools: &[],
                output_schema: &schema,
            },
        };
        let http = OpenAiRequestBuilder::new(Secret::new("test-api-key"), None)
            .build(request)
            .unwrap();

        assert_eq!(http.body["input"][0]["type"], "reasoning");
        assert_eq!(http.body["input"][0]["id"], "rs-1");
        assert_eq!(http.body["input"][1]["type"], "function_call");
        assert_eq!(http.body["input"][1]["id"], "fc-1");
        assert_eq!(http.body["input"][2]["type"], "function_call_output");
        assert_eq!(http.body["input"][2]["call_id"], "call-1");
    }

    #[test]
    fn parses_structured_output_tool_response() {
        let body = json!({
            "output": [{
                "id": "fc-structured",
                "call_id": "call-structured",
                "type": "function_call",
                "name": STRUCTURED_OUTPUT_TOOL_NAME,
                "arguments": "{\"summary\":\"looks good\",\"findings\":[]}"
            }],
            "usage": {
                "input_tokens": 80,
                "output_tokens": 40
            }
        });

        let result = parse_success_check(&body).unwrap();

        assert_eq!(result.usage.input_tokens, 80);
        assert_eq!(result.usage.output_tokens, 40);
        let LlmResponse::CheckOutput(output) = result.response else {
            panic!("expected check output");
        };
        assert_eq!(output.summary, "looks good");
        assert_eq!(output.findings, vec![]);
    }

    #[test]
    fn parses_json_content_as_structured_response() {
        let body = json!({
            "output": [{
                "id": "msg-1",
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
        });

        let result = parse_success_check(&body).unwrap();

        let LlmResponse::CheckOutput(output) = result.response else {
            panic!("expected check output");
        };
        assert_eq!(output.summary, "content json");
        assert_eq!(output.findings, vec![]);
    }

    #[test]
    fn parses_text_content_as_text_response() {
        let body = json!({
            "output": [{
                "id": "msg-1",
                "type": "message",
                "role": "assistant",
                "content": [{
                    "type": "output_text",
                    "text": "This PR updates the review flow."
                }]
            }],
            "usage": {
                "input_tokens": 10,
                "output_tokens": 5
            }
        });

        let result = parse_success_text(&body).unwrap();

        assert_eq!(
            result,
            LlmCallResult {
                response: LlmResponse::Text("This PR updates the review flow.".to_string()),
                usage: RawUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                },
            }
        );
    }

    #[test]
    fn invalid_success_response_is_permanent_error() {
        let body = json!({
            "output": [{
                "id": "fc-1",
                "call_id": "call-1",
                "type": "function_call",
                "name": "commit_diff",
                "arguments": "{"
            }],
            "usage": {
                "input_tokens": 100,
                "output_tokens": 25
            }
        });

        let error = parse_success_check(&body).unwrap_err();

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
