mod error;
mod http;
mod request;
mod response;

use reqwest::StatusCode;
use reqwest::header::HeaderMap;
use serde_json::Value;

pub use error::LlmCallError;
#[expect(unused_imports)]
pub use request::{ConversationTurn, LlmRequest, ToolSpec};
#[expect(unused_imports)]
pub use response::{LlmCallResult, LlmResponse, RawUsage, ToolCall};

#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub url: String,
    pub headers: HeaderMap,
    pub body: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub status: StatusCode,
    pub body: Value,
}

#[expect(dead_code)]
pub trait LlmProvider {
    fn build_request(
        &self,
        request: LlmRequest<'_>,
        is_last_request: bool,
    ) -> Result<Request, LlmCallError>;

    fn parse_response(&self, response: Response) -> Result<LlmCallResult, LlmCallError>;
}
