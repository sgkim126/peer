mod error;
mod http;
mod request;
mod response;

pub use error::LlmCallError;
#[expect(unused_imports)]
pub use request::{ConversationTurn, LlmRequest, ToolSpec};
#[expect(unused_imports)]
pub use response::{LlmCallResult, LlmResponse, RawUsage, ToolCall};

#[expect(dead_code)]
pub trait LlmProvider {
    async fn send(&self, request: LlmRequest<'_>) -> Result<LlmCallResult, LlmCallError>;
}
