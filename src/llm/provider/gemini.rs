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
pub struct GeminiRequestBuilder {
    api_key: Secret,
    base_url: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GeminiHttpRequest {
    pub url: String,
    pub api_key: Secret,
    pub body: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct GeminiProvider {
    client: ProviderHttpClient,
    request_builder: GeminiRequestBuilder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GeminiResponseParser;

const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com";
// FIXME: make the timeout configurable
const DEFAULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

impl GeminiProvider {
    pub fn from_env(
        api_key_env: &str,
        base_url: Option<&str>,
        console: Console,
    ) -> Result<Self, LlmCallError> {
        let request_builder = GeminiRequestBuilder::from_env(api_key_env, base_url)?;
        let client = reqwest::Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .map_err(|error| LlmCallError::Permanent {
                message: "failed to build Gemini HTTP client".to_string(),
                source: Box::new(error),
            })?;

        Ok(Self {
            client: ProviderHttpClient::new(client, console, "gemini", "Gemini"),
            request_builder,
        })
    }
}

impl LlmProvider for GeminiProvider {
    async fn send(&self, request: LlmRequest<'_>) -> Result<LlmCallResult, LlmCallError> {
        let output_mode = request.output_mode;
        let http = self.request_builder.build(request)?;
        let request = self
            .client
            .post(&http.url)
            .header("x-goog-api-key", http.api_key.expose_secret());
        let response = self.client.send_json(request, &http.body).await?;

        if response.status.is_success() {
            GeminiResponseParser::parse_success(&response.body, output_mode)
        } else {
            Err(GeminiResponseParser::parse_error(
                response.status.as_u16(),
                &response.body,
            ))
        }
    }
}

impl GeminiRequestBuilder {
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

    pub fn build(&self, request: LlmRequest<'_>) -> Result<GeminiHttpRequest, LlmCallError> {
        let (system_instruction, contents) = contents(request.conversation)?;
        let mut body = request_body(request.output_mode, contents);
        if let Some(system_instruction) = system_instruction {
            body["systemInstruction"] = system_instruction;
        }

        Ok(GeminiHttpRequest {
            url: format!(
                "{}/v1beta/{}:generateContent",
                self.base_url,
                model_path(request.model)
            ),
            api_key: self.api_key.clone(),
            body,
        })
    }
}

impl GeminiResponseParser {
    pub fn parse_success(
        body: &serde_json::Value,
        output_mode: LlmOutputMode<'_>,
    ) -> Result<LlmCallResult, LlmCallError> {
        let parts = body
            .pointer("/candidates/0/content/parts")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| permanent_parse_error("missing candidates[0].content.parts"))?;
        let usage = parse_usage(body)?;
        let response = parse_response(parts, output_mode)?;

        Ok(LlmCallResult { response, usage })
    }

    pub fn parse_error(status: u16, body: &serde_json::Value) -> LlmCallError {
        let message = error_message(body);
        if is_context_overflow(status, &message) {
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
}

fn parse_usage(body: &serde_json::Value) -> Result<RawUsage, LlmCallError> {
    Ok(RawUsage {
        input_tokens: required_u32(body, "/usageMetadata/promptTokenCount")?,
        output_tokens: required_u32(body, "/usageMetadata/candidatesTokenCount")?,
    })
}

fn parse_response(
    parts: &[serde_json::Value],
    output_mode: LlmOutputMode<'_>,
) -> Result<LlmResponse, LlmCallError> {
    match output_mode {
        LlmOutputMode::Check { .. } => parse_check_response(parts),
        LlmOutputMode::Text => parse_text_response(parts),
    }
}

fn parse_check_response(parts: &[serde_json::Value]) -> Result<LlmResponse, LlmCallError> {
    let tool_calls = parts
        .iter()
        .filter_map(|part| part.get("functionCall"))
        .enumerate()
        .map(|(index, value)| parse_tool_call(index, value))
        .collect::<Result<Vec<_>, _>>()?;

    if !tool_calls.is_empty() {
        if let Some(check_output) = check_output_response(&tool_calls)? {
            return Ok(LlmResponse::CheckOutput(check_output));
        }

        return Ok(LlmResponse::ToolCalls(tool_calls));
    }

    let text = parts
        .iter()
        .find_map(|part| part.get("text").and_then(serde_json::Value::as_str))
        .ok_or_else(|| permanent_parse_error("missing text or functionCall part"))?;
    let value = serde_json::from_str(text).map_err(|error| LlmCallError::Permanent {
        message: "failed to parse assistant text as JSON".to_string(),
        source: Box::new(error),
    })?;

    Ok(LlmResponse::CheckOutput(parse_check_output(value)?))
}

fn parse_text_response(_parts: &[serde_json::Value]) -> Result<LlmResponse, LlmCallError> {
    unimplemented!()
}

fn parse_tool_call(index: usize, value: &serde_json::Value) -> Result<ToolCall, LlmCallError> {
    let name = required_string(value, "/name")?.to_string();
    let arguments = value
        .get("args")
        .cloned()
        .ok_or_else(|| permanent_parse_error("missing function call args"))?;

    Ok(ToolCall {
        id: gemini_call_id(index, &name),
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
        .unwrap_or("Gemini request failed")
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
        source: Box::new(GeminiParseError(message.clone())),
        message,
    }
}

#[derive(Debug)]
struct GeminiParseError(String);

impl fmt::Display for GeminiParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for GeminiParseError {}

#[derive(Debug)]
struct GeminiStatusError {
    status: u16,
    message: String,
}

impl fmt::Display for GeminiStatusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Gemini returned HTTP {}: {}", self.status, self.message)
    }
}

impl std::error::Error for GeminiStatusError {}

fn request_body(
    output_mode: LlmOutputMode<'_>,
    contents: Vec<serde_json::Value>,
) -> serde_json::Value {
    match output_mode {
        LlmOutputMode::Check {
            tools: tool_specs,
            output_schema,
        } => json!({
            "contents": contents,
            "tools": tools(tool_specs, output_schema),
            "toolConfig": {
                "functionCallingConfig": {
                    "mode": "AUTO"
                }
            },
            "generationConfig": {
                "responseMimeType": "application/json"
            }
        }),
        LlmOutputMode::Text => {
            unimplemented!()
        }
    }
}

fn contents(
    conversation: &[ConversationTurn],
) -> Result<(Option<serde_json::Value>, Vec<serde_json::Value>), LlmCallError> {
    let mut system = Vec::new();
    let mut contents = Vec::new();

    for turn in conversation {
        match turn {
            ConversationTurn::System(content) => system.push(content.clone()),
            _ => contents.push(content(turn)?),
        }
    }

    let system_instruction = (!system.is_empty()).then(|| {
        json!({
            "parts": [{
                "text": system.join("\n\n"),
            }]
        })
    });
    Ok((system_instruction, contents))
}

fn content(turn: &ConversationTurn) -> Result<serde_json::Value, LlmCallError> {
    match turn {
        ConversationTurn::System(_) => unreachable!("system turns are handled separately"),
        ConversationTurn::User(content) => Ok(json!({
            "role": "user",
            "parts": [{
                "text": content,
            }],
        })),
        ConversationTurn::AssistantToolCalls(tool_calls) => Ok(json!({
            "role": "model",
            "parts": tool_calls.iter().map(gemini_function_call).collect::<Vec<_>>(),
        })),
        ConversationTurn::ToolResult { call_id, result } => Ok(json!({
            "role": "user",
            "parts": [{
                "functionResponse": {
                    "name": gemini_call_name(call_id)?,
                    "response": result,
                }
            }],
        })),
        ConversationTurn::AssistantCheckOutput(output) => Ok(json!({
            "role": "model",
            "parts": [{
                "text": serde_json::to_string(output).map_err(|error| LlmCallError::Permanent {
                    message: "failed to encode assistant check output".to_string(),
                    source: Box::new(error),
                })?,
            }],
        })),
    }
}

fn gemini_function_call(tool_call: &ToolCall) -> serde_json::Value {
    json!({
        "functionCall": {
            "name": tool_call.name,
            "args": tool_call.arguments,
        }
    })
}

fn tools(tool_specs: &[ToolSpec], output_schema: &serde_json::Value) -> Vec<serde_json::Value> {
    vec![json!({
        "functionDeclarations": tool_specs
            .iter()
            .map(tool)
            .chain(std::iter::once(structured_output_tool(output_schema)))
            .collect::<Vec<_>>()
    })]
}

fn tool(spec: &ToolSpec) -> serde_json::Value {
    json!({
        "name": spec.name,
        "description": spec.description,
        "parameters": spec.parameters,
    })
}

const STRUCTURED_OUTPUT_TOOL_NAME: &str = "submit_check_result";
fn structured_output_tool(output_schema: &serde_json::Value) -> serde_json::Value {
    json!({
        "name": STRUCTURED_OUTPUT_TOOL_NAME,
        "description": "Submit the final structured check result.",
        "parameters": output_schema,
    })
}

fn model_path(model: &str) -> String {
    if model.starts_with("models/") {
        model.to_string()
    } else {
        format!("models/{model}")
    }
}

fn gemini_call_id(index: usize, name: &str) -> String {
    format!("gemini:{index}:{name}")
}

fn gemini_call_name(call_id: &str) -> Result<&str, LlmCallError> {
    let Some(name) = call_id
        .strip_prefix("gemini:")
        .and_then(|value| value.split_once(':').map(|(_, name)| name))
    else {
        return Err(permanent_parse_error(format!(
            "invalid Gemini tool call id: {call_id}"
        )));
    };
    Ok(name)
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
        GeminiResponseParser::parse_success(
            body,
            LlmOutputMode::Check {
                tools: &[],
                output_schema: &schema,
            },
        )
    }

    #[test]
    fn builds_gemini_request_body_with_contents_tools_and_structured_output_tool() {
        let builder = GeminiRequestBuilder::new(
            Secret::new("test-api-key"),
            Some("https://gemini.example.test/"),
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
            model: "gemini-2.5-pro",
            conversation: &conversation,
            output_mode: LlmOutputMode::Check {
                tools: &[extract_tool],
                output_schema: &schema,
            },
        };

        let http = builder.build(request).unwrap();

        assert_eq!(
            http.url,
            "https://gemini.example.test/v1beta/models/gemini-2.5-pro:generateContent"
        );
        assert_eq!(http.api_key.expose_secret(), "test-api-key");
        assert_eq!(
            http.body["systemInstruction"]["parts"][0]["text"],
            "You review code."
        );
        assert_eq!(http.body["contents"][0]["role"], "user");
        assert_eq!(
            http.body["contents"][0]["parts"][0]["text"],
            "Check abc1234."
        );
        assert_eq!(
            http.body["tools"][0]["functionDeclarations"][0]["name"],
            "commit_diff"
        );
        assert_eq!(
            http.body["tools"][0]["functionDeclarations"][0]["parameters"],
            extract_tool_parameters
        );
        assert_eq!(
            http.body["tools"][0]["functionDeclarations"][1]["name"],
            STRUCTURED_OUTPUT_TOOL_NAME
        );
        assert_eq!(
            http.body["tools"][0]["functionDeclarations"][1]["parameters"],
            schema
        );
        assert_eq!(
            http.body["toolConfig"]["functionCallingConfig"]["mode"],
            "AUTO"
        );
        assert_eq!(
            http.body["generationConfig"]["responseMimeType"],
            "application/json"
        );
    }

    #[test]
    fn encodes_assistant_tool_calls_and_tool_results() {
        let builder = GeminiRequestBuilder::new(Secret::new("test-api-key"), None);
        let schema = output_schema();
        let conversation = [
            ConversationTurn::AssistantToolCalls(vec![ToolCall {
                id: "gemini:0:commit_diff".to_string(),
                name: "commit_diff".to_string(),
                arguments: json!({ "hash": "abc1234" }),
            }]),
            ConversationTurn::ToolResult {
                call_id: "gemini:0:commit_diff".to_string(),
                result: json!({ "diff": "+hello" }),
            },
        ];
        let request = LlmRequest {
            model: "models/gemini-2.5-pro",
            conversation: &conversation,
            output_mode: LlmOutputMode::Check {
                tools: &[],
                output_schema: &schema,
            },
        };

        let http = builder.build(request).unwrap();

        assert_eq!(
            http.url,
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-pro:generateContent"
        );
        assert_eq!(http.body["contents"][0]["role"], "model");
        assert_eq!(
            http.body["contents"][0]["parts"][0]["functionCall"]["name"],
            "commit_diff"
        );
        assert_eq!(
            http.body["contents"][0]["parts"][0]["functionCall"]["args"]["hash"],
            "abc1234"
        );
        assert_eq!(http.body["contents"][1]["role"], "user");
        assert_eq!(
            http.body["contents"][1]["parts"][0]["functionResponse"]["name"],
            "commit_diff"
        );
        assert_eq!(
            http.body["contents"][1]["parts"][0]["functionResponse"]["response"]["diff"],
            "+hello"
        );
    }

    #[test]
    fn missing_api_key_is_permanent_error() {
        let name = "PEER_TEST_MISSING_GEMINI_API_KEY_4B9D5E7C9A1F";

        let error = GeminiRequestBuilder::from_env(name, None).unwrap_err();

        assert!(matches!(error, LlmCallError::Permanent { .. }));
        assert!(error.to_string().contains(name));
    }

    #[test]
    fn provider_from_env_fails_when_api_key_is_missing() {
        let name = "PEER_TEST_MISSING_GEMINI_PROVIDER_API_KEY_92F4A1C8D3";

        let error = GeminiProvider::from_env(name, None, Console::default()).unwrap_err();

        assert!(matches!(error, LlmCallError::Permanent { .. }));
        assert!(error.to_string().contains(name));
        assert!(std::error::Error::source(&error).is_some());
    }

    #[test]
    fn debug_redacts_api_key() {
        let builder = GeminiRequestBuilder::new(Secret::new("test-api-key"), None);
        let schema = output_schema();
        let request = LlmRequest {
            model: "gemini-2.5-pro",
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
    fn parses_function_call_response() {
        let body = json!({
            "candidates": [{
                "content": {
                    "parts": [{
                        "functionCall": {
                            "name": "commit_diff",
                            "args": { "hash": "abc1234" }
                        }
                    }]
                }
            }],
            "usageMetadata": {
                "promptTokenCount": 100,
                "candidatesTokenCount": 25
            }
        });

        let result = parse_success_check(&body).unwrap();

        assert_eq!(result.usage.input_tokens, 100);
        assert_eq!(result.usage.output_tokens, 25);
        let LlmResponse::ToolCalls(tool_calls) = result.response else {
            panic!("expected tool calls");
        };
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "gemini:0:commit_diff");
        assert_eq!(tool_calls[0].name, "commit_diff");
        assert_eq!(tool_calls[0].arguments, json!({ "hash": "abc1234" }));
    }

    #[test]
    fn parses_structured_output_function_response() {
        let body = json!({
            "candidates": [{
                "content": {
                    "parts": [{
                        "functionCall": {
                            "name": STRUCTURED_OUTPUT_TOOL_NAME,
                            "args": {
                                "summary": "looks good",
                                "findings": [],
                                "confidence": 0.9
                            }
                        }
                    }]
                }
            }],
            "usageMetadata": {
                "promptTokenCount": 80,
                "candidatesTokenCount": 40
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
        assert_eq!(output.confidence.as_f64(), 0.9);
    }

    #[test]
    fn parses_json_text_as_structured_response() {
        let body = json!({
            "candidates": [{
                "content": {
                    "parts": [{
                        "text": "{\"summary\":\"content json\",\"findings\":[],\"confidence\":0.8}"
                    }]
                }
            }],
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 5
            }
        });

        let result = parse_success_check(&body).unwrap();

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
            "candidates": [{
                "content": {
                    "parts": [{
                        "text": "{"
                    }]
                }
            }],
            "usageMetadata": {
                "promptTokenCount": 100,
                "candidatesTokenCount": 25
            }
        });

        let error = parse_success_check(&body).unwrap_err();

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

        let error = GeminiResponseParser::parse_error(400, &body);

        assert!(matches!(error, LlmCallError::ContextOverflow { .. }));
    }

    #[test]
    fn parses_retryable_error_as_transient() {
        let body = json!({
            "error": {
                "message": "rate limit exceeded"
            }
        });

        let error = GeminiResponseParser::parse_error(429, &body);

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

        let error = GeminiResponseParser::parse_error(401, &body);

        assert!(matches!(error, LlmCallError::Permanent { .. }));
        assert!(error.to_string().contains("invalid API key"));
        assert!(std::error::Error::source(&error).is_some());
    }
}
