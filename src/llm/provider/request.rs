use super::response::ToolCall;
use crate::llm::result::CheckOutput;

#[derive(Debug, Clone, PartialEq)]
pub enum ConversationTurn {
    System(String),
    User(String),
    /// Tool calls requested by the assistant. Each call is matched with a
    /// subsequent [`ConversationTurn::ToolResult`] by its ID.
    AssistantToolCalls(Vec<ToolCall>),
    /// The result of executing a tool requested by the assistant.
    /// `call_id` identifies the corresponding call in
    /// [`ConversationTurn::AssistantToolCalls`].
    ToolResult {
        call_id: String,
        result: serde_json::Value,
    },
    /// A final check output produced by the assistant. The agent may retain
    /// it in the conversation and request further analysis when its
    /// confidence is below the configured threshold.
    AssistantCheckOutput(CheckOutput),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

pub struct LlmRequest<'a> {
    pub model: &'a str,
    pub conversation: &'a [ConversationTurn],
    pub output_mode: LlmOutputMode<'a>,
}

#[derive(Debug, Clone, Copy)]
pub enum LlmOutputMode<'a> {
    Check {
        tools: &'a [ToolSpec],
        output_schema: &'a serde_json::Value,
    },
    Text,
}
