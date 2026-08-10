mod result;

pub use self::result::{
    CheckError, CheckResult, CheckTarget, Finding, LlmModelUsage, LlmUsage, Severity,
};

#[cfg(test)]
pub use self::result::FileLocation;

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
}
