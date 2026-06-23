mod error;
mod mistral;
mod request;
mod response;

pub use error::LlmCallError;
pub use request::{ConversationTurn, LlmRequest, ToolSpec};
pub use response::{LlmCallResult, LlmResponse, RawUsage, ToolCall};

#[allow(dead_code)]
pub trait LlmProvider {
    async fn send(&self, request: LlmRequest<'_>) -> Result<LlmCallResult, LlmCallError>;
}
