mod anthropic;
mod debug;
mod error;
mod gemini;
mod mistral;
mod openai;
mod request;
mod response;

use std::fmt;

use crate::console::Console;

pub use error::LlmCallError;
pub use request::{ConversationTurn, LlmRequest, ToolSpec};
pub use response::{LlmCallResult, LlmResponse, RawUsage, ToolCall};

pub trait LlmProvider {
    async fn send(&self, request: LlmRequest<'_>) -> Result<LlmCallResult, LlmCallError>;
}

enum Provider {
    Anthropic(anthropic::AnthropicProvider),
    Gemini(gemini::GeminiProvider),
    Mistral(mistral::MistralProvider),
    OpenAi(openai::OpenAiProvider),
}

impl LlmProvider for Provider {
    async fn send(&self, request: LlmRequest<'_>) -> Result<LlmCallResult, LlmCallError> {
        match self {
            Self::Anthropic(provider) => provider.send(request).await,
            Self::Gemini(provider) => provider.send(request).await,
            Self::Mistral(provider) => provider.send(request).await,
            Self::OpenAi(provider) => provider.send(request).await,
        }
    }
}

pub fn create_provider(
    name: &str,
    api_key_env: &str,
    base_url: Option<&str>,
    console: Console,
) -> Result<impl LlmProvider, ProviderCreationError> {
    match name {
        "anthropic" => Ok(
            anthropic::AnthropicProvider::from_env(api_key_env, base_url, console)
                .map(Provider::Anthropic)?,
        ),
        "gemini" => Ok(
            gemini::GeminiProvider::from_env(api_key_env, base_url, console)
                .map(Provider::Gemini)?,
        ),
        "mistral" => Ok(
            mistral::MistralProvider::from_env(api_key_env, base_url, console)
                .map(Provider::Mistral)?,
        ),
        "openai" => Ok(
            openai::OpenAiProvider::from_env(api_key_env, base_url, console)
                .map(Provider::OpenAi)?,
        ),
        _ => Err(ProviderCreationError::Unsupported {
            name: name.to_string(),
        }),
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
    fn from(err: LlmCallError) -> Self {
        Self::Initialization(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_rejects_unsupported_provider() {
        let Err(error) = create_provider("unknown", "UNUSED_API_KEY", None, Console::default())
        else {
            panic!("expected unsupported provider error");
        };

        assert!(matches!(
            error,
            ProviderCreationError::Unsupported { name } if name == "unknown"
        ));
    }

    #[test]
    fn factory_supports_mistral_provider() {
        let name = "PEER_TEST_MISSING_MISTRAL_FACTORY_API_KEY_7A2C9D4E1B";

        let Err(error) = create_provider("mistral", name, None, Console::default()) else {
            panic!("expected provider initialization error");
        };

        assert!(matches!(error, ProviderCreationError::Initialization(_)));
        assert!(error.to_string().contains(name));
    }

    #[test]
    fn factory_supports_openai_provider() {
        let name = "PEER_TEST_MISSING_OPENAI_FACTORY_API_KEY_7A2C9D4E1B";

        let Err(error) = create_provider("openai", name, None, Console::default()) else {
            panic!("expected provider initialization error");
        };

        assert!(matches!(error, ProviderCreationError::Initialization(_)));
        assert!(error.to_string().contains(name));
    }

    #[test]
    fn factory_supports_anthropic_provider() {
        let name = "PEER_TEST_MISSING_ANTHROPIC_FACTORY_API_KEY_7A2C9D4E1B";

        let Err(error) = create_provider("anthropic", name, None, Console::default()) else {
            panic!("expected provider initialization error");
        };

        assert!(matches!(error, ProviderCreationError::Initialization(_)));
        assert!(error.to_string().contains(name));
    }

    #[test]
    fn factory_supports_gemini_provider() {
        let name = "PEER_TEST_MISSING_GEMINI_FACTORY_API_KEY_7A2C9D4E1B";

        let Err(error) = create_provider("gemini", name, None, Console::default()) else {
            panic!("expected provider initialization error");
        };

        assert!(matches!(error, ProviderCreationError::Initialization(_)));
        assert!(error.to_string().contains(name));
    }
}
