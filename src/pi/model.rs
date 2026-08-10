use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRef {
    provider: String,
    model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRefError;

impl ModelRef {
    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn model(&self) -> &str {
        &self.model
    }
}

impl FromStr for ModelRef {
    type Err = ModelRefError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let Some((provider, model)) = value.split_once('/') else {
            return Err(ModelRefError);
        };
        if provider.is_empty()
            || model.is_empty()
            || provider.trim() != provider
            || model.trim() != model
        {
            return Err(ModelRefError);
        }
        Ok(Self {
            provider: provider.to_string(),
            model: model.to_string(),
        })
    }
}

impl fmt::Display for ModelRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.provider, self.model)
    }
}

impl fmt::Display for ModelRefError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("model must use provider/model format")
    }
}

impl std::error::Error for ModelRefError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_provider_and_model() {
        let model: ModelRef = "mistral/mistral-medium-3-5".parse().unwrap();
        assert_eq!(model.provider(), "mistral");
        assert_eq!(model.model(), "mistral-medium-3-5");
        assert_eq!(model.to_string(), "mistral/mistral-medium-3-5");
    }

    #[test]
    fn rejects_unscoped_models() {
        assert_eq!("mistral-medium-3-5".parse::<ModelRef>(), Err(ModelRefError));
    }

    #[test]
    fn keeps_further_segments_in_the_model_id() {
        let model: ModelRef = "openrouter/anthropic/claude".parse().unwrap();
        assert_eq!(model.provider(), "openrouter");
        assert_eq!(model.model(), "anthropic/claude");
    }

    #[test]
    fn rejects_empty_and_untrimmed_parts() {
        for value in [
            "",
            "/",
            "/model",
            "provider/",
            " provider/model",
            "provider/model ",
        ] {
            assert_eq!(value.parse::<ModelRef>(), Err(ModelRefError), "{value:?}");
        }
    }
}
