use super::response::ToolCall;

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(not(test), expect(dead_code))]
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
    pub tools: &'a [ToolSpec],
}
