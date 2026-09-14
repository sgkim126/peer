use std::collections::BTreeMap;

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
    pub fn zero(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self::from(vec![LlmModelUsage {
            provider: provider.into(),
            model: model.into(),
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_usd: 0.0,
        }])
    }

    pub fn iter(&self) -> std::slice::Iter<'_, LlmModelUsage> {
        self.models.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }
}

impl From<Vec<LlmModelUsage>> for LlmUsage {
    /// Sums usage for each provider/model pair and sorts the result by provider and model.
    fn from(models: Vec<LlmModelUsage>) -> Self {
        let mut totals = BTreeMap::<(String, String), LlmModelUsage>::new();
        for usage in models {
            let key = (usage.provider.clone(), usage.model.clone());
            match totals.entry(key) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(usage);
                }
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    let total = entry.get_mut();
                    total.input_tokens += usage.input_tokens;
                    total.output_tokens += usage.output_tokens;
                    total.cache_read_tokens += usage.cache_read_tokens;
                    total.cache_write_tokens += usage.cache_write_tokens;
                    total.cost_usd += usage.cost_usd;
                }
            }
        }
        let models: Vec<_> = totals.into_values().collect();
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
    fn zero_usage_preserves_provider_and_model() {
        let usage = LlmUsage::zero("openrouter/team", "anthropic/claude");

        assert_eq!(
            usage.iter().collect::<Vec<_>>(),
            [&LlmModelUsage {
                provider: "openrouter/team".into(),
                model: "anthropic/claude".into(),
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: 0.0,
            }]
        );
    }

    #[test]
    fn pi_usage_preserves_cache_tokens_and_model_costs() {
        let usage = LlmUsage::from(vec![LlmModelUsage {
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

    #[test]
    fn merges_and_orders_usage_by_provider_and_model() {
        let first = LlmModelUsage {
            provider: "provider/team".into(),
            model: "model".into(),
            input_tokens: 10,
            output_tokens: 2,
            cache_read_tokens: 3,
            cache_write_tokens: 4,
            cost_usd: 0.125,
        };
        let second = LlmModelUsage {
            provider: "provider".into(),
            model: "team/model".into(),
            ..first.clone()
        };

        let usage = LlmUsage::from(vec![first.clone(), second.clone(), first]);

        assert_eq!(
            usage.iter().cloned().collect::<Vec<_>>(),
            vec![
                second,
                LlmModelUsage {
                    provider: "provider/team".into(),
                    model: "model".into(),
                    input_tokens: 20,
                    output_tokens: 4,
                    cache_read_tokens: 6,
                    cache_write_tokens: 8,
                    cost_usd: 0.25,
                },
            ]
        );
    }
}
