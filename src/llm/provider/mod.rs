mod anthropic;
mod error;
mod gemini;
mod http;
mod mistral;
mod openai;
mod request;
mod response;

use reqwest::StatusCode;
use reqwest::header::HeaderMap;
use serde_json::Value;

#[expect(unused_imports)]
pub use anthropic::AnthropicProvider;
pub use error::LlmCallError;
#[expect(unused_imports)]
pub use gemini::GeminiProvider;
pub use http::ProviderHttpClient;
#[expect(unused_imports)]
pub use mistral::MistralProvider;
#[expect(unused_imports)]
pub use openai::OpenAiProvider;
pub use request::{ConversationTurn, LlmRequest, ToolSpec};
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
pub trait LlmTransport {
    async fn send(&self, request: Request) -> Result<Response, LlmCallError>;
}

impl LlmTransport for ProviderHttpClient {
    async fn send(&self, request: Request) -> Result<Response, LlmCallError> {
        ProviderHttpClient::send(self, request).await
    }
}

#[cfg_attr(not(test), expect(dead_code))]
pub trait LlmProvider {
    fn build_request(
        &self,
        request: LlmRequest<'_>,
        is_last_request: bool,
    ) -> Result<Request, LlmCallError>;

    fn parse_response(&self, response: Response) -> Result<LlmCallResult, LlmCallError>;
}
