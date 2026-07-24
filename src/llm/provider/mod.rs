mod anthropic;
mod error;
mod gemini;
mod http;
mod mistral;
mod openai;
mod request;
mod response;

use std::fmt;

use reqwest::StatusCode;
use reqwest::header::HeaderMap;
use serde_json::Value;

pub use anthropic::AnthropicProvider;
pub use error::LlmCallError;
pub use gemini::GeminiProvider;
pub use http::ProviderHttpClient;
pub use mistral::MistralProvider;
pub use openai::OpenAiProvider;
pub use request::{ConversationTurn, LlmRequest, ToolSpec};
pub use response::{LlmCallResult, LlmResponse, RawUsage, ToolCall};

/// A configured provider paired with the HTTP transport used to call it.
///
/// The agent deliberately keeps provider request construction separate from
/// transport.  This wrapper gives command handlers one value to construct
/// from configuration while preserving that split internally.
pub struct ProviderRuntime {
    provider: Provider,
    transport: ProviderHttpClient,
}

#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
#[value(rename_all = "lower")]
pub enum ProviderKind {
    Anthropic,
    Gemini,
    Mistral,
    OpenAi,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::Gemini => "gemini",
            Self::Mistral => "mistral",
            Self::OpenAi => "openai",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        clap::ValueEnum::from_str(name, false).ok()
    }
}

impl ProviderRuntime {
    pub fn try_new(
        name: &str,
        api_key_env: &str,
        base_url: Option<&str>,
        console: crate::console::Console,
    ) -> Result<Self, ProviderCreationError> {
        let provider_kind =
            ProviderKind::from_name(name).ok_or_else(|| ProviderCreationError::Unsupported {
                name: name.to_string(),
            })?;
        let provider = match provider_kind {
            ProviderKind::Anthropic => {
                AnthropicProvider::from_env(api_key_env, base_url).map(Provider::Anthropic)?
            }
            ProviderKind::Gemini => {
                GeminiProvider::from_env(api_key_env, base_url).map(Provider::Gemini)?
            }
            ProviderKind::Mistral => {
                MistralProvider::from_env(api_key_env, base_url).map(Provider::Mistral)?
            }
            ProviderKind::OpenAi => {
                OpenAiProvider::from_env(api_key_env, base_url).map(Provider::OpenAi)?
            }
        };

        Ok(Self {
            transport: ProviderHttpClient::new(reqwest::Client::new(), console, provider.name()),
            provider,
        })
    }

    pub fn into_parts(self) -> (Provider, ProviderHttpClient) {
        (self.provider, self.transport)
    }
}

pub enum Provider {
    Anthropic(AnthropicProvider),
    Gemini(GeminiProvider),
    Mistral(MistralProvider),
    OpenAi(OpenAiProvider),
}

impl Provider {
    fn name(&self) -> &'static str {
        match self {
            Self::Anthropic(_) => "anthropic",
            Self::Gemini(_) => "gemini",
            Self::Mistral(_) => "mistral",
            Self::OpenAi(_) => "openai",
        }
    }
}

impl LlmProvider for Provider {
    fn build_request(
        &self,
        request: LlmRequest<'_>,
        is_last_request: bool,
    ) -> Result<Request, LlmCallError> {
        match self {
            Self::Anthropic(provider) => provider.build_request(request, is_last_request),
            Self::Gemini(provider) => provider.build_request(request, is_last_request),
            Self::Mistral(provider) => provider.build_request(request, is_last_request),
            Self::OpenAi(provider) => provider.build_request(request, is_last_request),
        }
    }

    fn parse_response(&self, response: Response) -> Result<LlmCallResult, LlmCallError> {
        match self {
            Self::Anthropic(provider) => provider.parse_response(response),
            Self::Gemini(provider) => provider.parse_response(response),
            Self::Mistral(provider) => provider.parse_response(response),
            Self::OpenAi(provider) => provider.parse_response(response),
        }
    }
}

#[derive(Debug)]
pub enum ProviderCreationError {
    Unsupported { name: String },
    Initialization(LlmCallError),
}

impl fmt::Display for ProviderCreationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported { name } => write!(f, "unsupported LLM provider: {name}"),
            Self::Initialization(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ProviderCreationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Unsupported { .. } => None,
            Self::Initialization(error) => Some(error),
        }
    }
}

impl From<LlmCallError> for ProviderCreationError {
    fn from(error: LlmCallError) -> Self {
        Self::Initialization(error)
    }
}

#[cfg(test)]
mod runtime_tests {
    use super::*;

    #[test]
    fn factory_rejects_unknown_provider() {
        let Err(error) = ProviderRuntime::try_new(
            "unknown",
            "UNUSED",
            None,
            crate::console::Console::default(),
        ) else {
            panic!("unknown provider must be rejected");
        };

        assert!(matches!(
            error,
            ProviderCreationError::Unsupported { name } if name == "unknown"
        ));
    }

    #[test]
    fn factory_reports_missing_provider_key() {
        let Err(error) = ProviderRuntime::try_new(
            "mistral",
            "PEER_TEST_MISSING_PROVIDER_RUNTIME_KEY",
            None,
            crate::console::Console::default(),
        ) else {
            panic!("missing provider key must fail initialization");
        };

        assert!(matches!(error, ProviderCreationError::Initialization(_)));
    }
}

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

pub trait LlmTransport {
    async fn send(&self, request: Request) -> Result<Response, LlmCallError>;
}

impl LlmTransport for ProviderHttpClient {
    async fn send(&self, request: Request) -> Result<Response, LlmCallError> {
        ProviderHttpClient::send(self, request).await
    }
}

pub trait LlmProvider {
    fn build_request(
        &self,
        request: LlmRequest<'_>,
        is_last_request: bool,
    ) -> Result<Request, LlmCallError>;

    fn parse_response(&self, response: Response) -> Result<LlmCallResult, LlmCallError>;
}
