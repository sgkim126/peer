use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRef {
    provider: String,
    model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelRefError {
    InvalidProvider(String),
    InvalidModel(String),
}

impl ModelRef {
    pub fn try_new(
        provider: impl Into<String>,
        model: impl Into<String>,
    ) -> Result<Self, ModelRefError> {
        let provider = provider.into();
        if provider.is_empty() || provider.trim() != provider {
            return Err(ModelRefError::InvalidProvider(provider));
        }

        let model = model.into();
        if model.is_empty() || model.trim() != model {
            return Err(ModelRefError::InvalidModel(model));
        }

        Ok(Self { provider, model })
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn model(&self) -> &str {
        &self.model
    }
}

impl fmt::Display for ModelRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.provider, self.model)
    }
}

impl fmt::Display for ModelRefError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidProvider(provider) => write!(
                f,
                "Pi provider {provider} must not be empty or have surrounding whitespace"
            ),
            Self::InvalidModel(model) => write!(
                f,
                "Pi model {model} must not be empty or have surrounding whitespace"
            ),
        }
    }
}

impl std::error::Error for ModelRefError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructs_from_separate_provider_and_model_values() {
        let model = ModelRef::try_new("openrouter/team", "anthropic/claude").unwrap();
        assert_eq!(model.provider(), "openrouter/team");
        assert_eq!(model.model(), "anthropic/claude");
        assert_eq!(model.to_string(), "openrouter/team/anthropic/claude");
    }

    #[test]
    fn rejects_empty_provider() {
        assert_eq!(
            ModelRef::try_new("", "model"),
            Err(ModelRefError::InvalidProvider("".into()))
        );
    }

    #[test]
    fn rejects_provider_with_leading_whitespace() {
        assert_eq!(
            ModelRef::try_new(" provider", "model"),
            Err(ModelRefError::InvalidProvider(" provider".into()))
        );
    }

    #[test]
    fn rejects_provider_with_trailing_whitespace() {
        assert_eq!(
            ModelRef::try_new("provider ", "model"),
            Err(ModelRefError::InvalidProvider("provider ".into()))
        );
    }

    #[test]
    fn rejects_empty_model() {
        assert_eq!(
            ModelRef::try_new("provider", ""),
            Err(ModelRefError::InvalidModel("".into()))
        );
    }

    #[test]
    fn rejects_model_with_leading_whitespace() {
        assert_eq!(
            ModelRef::try_new("provider", " model"),
            Err(ModelRefError::InvalidModel(" model".into()))
        );
    }

    #[test]
    fn rejects_model_with_trailing_whitespace() {
        assert_eq!(
            ModelRef::try_new("provider", "model "),
            Err(ModelRefError::InvalidModel("model ".into()))
        );
    }
}
