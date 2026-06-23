use serde_json::json;

use super::{ConversationTurn, LlmCallError, LlmRequest, ToolCall, ToolSpec};
use crate::secret::Secret;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MistralRequestBuilder {
    api_key: Secret,
    base_url: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MistralHttpRequest {
    pub url: String,
    pub bearer_token: Secret,
    pub body: serde_json::Value,
}

const DEFAULT_BASE_URL: &str = "https://api.mistral.ai";

impl MistralRequestBuilder {
    #[cfg_attr(not(test), expect(dead_code))]
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

    #[cfg_attr(not(test), expect(dead_code))]
    pub fn build(&self, request: LlmRequest<'_>) -> Result<MistralHttpRequest, LlmCallError> {
        let messages = request
            .conversation
            .iter()
            .map(message)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(MistralHttpRequest {
            url: format!("{}/v1/chat/completions", self.base_url),
            bearer_token: self.api_key.clone(),
            body: json!({
                "model": request.model,
                "messages": messages,
                "tools": tools(request.tools, request.output_schema),
                "tool_choice": "auto",
            }),
        })
    }
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
            "tool_calls": tool_calls.iter().map(mistral_tool_call).collect::<Result<Vec<_>, _>>()?,
        })),
        ConversationTurn::ToolResult { call_id, result } => Ok(json!({
            "role": "tool",
            "tool_call_id": call_id,
            "content": result.to_string(),
        })),
    }
}

fn mistral_tool_call(tool_call: &ToolCall) -> Result<serde_json::Value, LlmCallError> {
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
    fn builds_mistral_request_body_with_messages_tools_and_structured_output_tool() {
        let builder = MistralRequestBuilder::new(
            Secret::new("test-api-key".to_string()),
            Some("https://mistral.example.test/"),
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
            model: "mistral-large-latest",
            conversation: &conversation,
            tools: &[extract_tool],
            output_schema: &schema,
        };

        let http = builder.build(request).unwrap();

        assert_eq!(http.url, "https://mistral.example.test/v1/chat/completions");
        assert_eq!(http.bearer_token.expose_secret(), "test-api-key");
        assert_eq!(http.body["model"], "mistral-large-latest");
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
    }

    #[test]
    fn encodes_assistant_tool_calls_and_tool_results() {
        let builder = MistralRequestBuilder::new(Secret::new("test-api-key".to_string()), None);
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
            model: "mistral-large-latest",
            conversation: &conversation,
            tools: &[],
            output_schema: &schema,
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
        let name = "PEER_TEST_MISSING_MISTRAL_API_KEY_4B9D5E7C9A1F";

        let error = MistralRequestBuilder::from_env(name, None).unwrap_err();

        assert!(matches!(error, LlmCallError::Permanent { .. }));
        assert!(error.to_string().contains(name));
    }

    #[test]
    fn debug_redacts_api_key_and_bearer_token() {
        let builder = MistralRequestBuilder::new(Secret::new("test-api-key".to_string()), None);
        let schema = output_schema();
        let request = LlmRequest {
            model: "mistral-large-latest",
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
}
