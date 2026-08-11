use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct LlmModelUsage {
    pub provider: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct LlmUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub cache_write_tokens: u64,
    pub cost_usd: f64,
    pub model: String,
    #[serde(default)]
    pub models: Vec<LlmModelUsage>,
}

impl LlmUsage {
    pub fn from_pi_models(models: Vec<LlmModelUsage>) -> Self {
        let input_tokens = models.iter().map(|usage| usage.input_tokens).sum();
        let output_tokens = models.iter().map(|usage| usage.output_tokens).sum();
        let cache_read_tokens = models.iter().map(|usage| usage.cache_read_tokens).sum();
        let cache_write_tokens = models.iter().map(|usage| usage.cache_write_tokens).sum();
        let cost_usd = models.iter().map(|usage| usage.cost_usd).sum();
        let model = match models.as_slice() {
            [usage] => format!("{}/{}", usage.provider, usage.model),
            [] => "unknown".to_string(),
            _ => "multiple".to_string(),
        };
        Self {
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            cost_usd,
            model,
            models,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pi_usage_preserves_cache_tokens_and_model_costs() {
        let usage = LlmUsage::from_pi_models(vec![LlmModelUsage {
            provider: "mistral".to_string(),
            model: "mistral-medium-3-5".to_string(),
            input_tokens: 100,
            output_tokens: 20,
            cache_read_tokens: 80,
            cache_write_tokens: 10,
            cost_usd: 0.012,
        }]);

        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 20);
        assert_eq!(usage.cache_read_tokens, 80);
        assert_eq!(usage.cache_write_tokens, 10);
        assert_eq!(usage.cost_usd, 0.012);
        assert_eq!(usage.model, "mistral/mistral-medium-3-5");
    }
}
