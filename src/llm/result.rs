use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};

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

#[derive(Debug, Default, Clone, PartialEq, Serialize)]
#[serde(transparent)]
pub struct LlmUsage {
    models: Vec<LlmModelUsage>,
}

impl LlmUsage {
    pub fn zero(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            models: vec![LlmModelUsage {
                provider: provider.into(),
                model: model.into(),
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: 0.0,
            }],
        }
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
        Self {
            models: totals.into_values().collect(),
        }
    }
}

impl<'de> Deserialize<'de> for LlmUsage {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Vec::<LlmModelUsage>::deserialize(deserializer).map(Self::from)
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
    fn usage_serializes_as_a_model_array() {
        let usage = LlmUsage::from(vec![LlmModelUsage {
            provider: "mistral".into(),
            model: "mistral-medium-3-5".into(),
            input_tokens: 100,
            output_tokens: 20,
            cache_read_tokens: 80,
            cache_write_tokens: 10,
            cost_usd: 0.012,
        }]);

        let json = serde_json::to_value(&usage).unwrap();
        assert_eq!(
            json,
            serde_json::json!([{
                "provider": "mistral",
                "model": "mistral-medium-3-5",
                "input_tokens": 100,
                "output_tokens": 20,
                "cache_read_tokens": 80,
                "cache_write_tokens": 10,
                "cost_usd": 0.012,
            }])
        );
        assert_eq!(serde_json::from_value::<LlmUsage>(json).unwrap(), usage);
    }

    #[test]
    fn deserialization_merges_and_orders_usage_by_provider_and_model() {
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
        let value = serde_json::to_value([first.clone(), second.clone(), first]).unwrap();

        let usage: LlmUsage = serde_json::from_value(value).unwrap();

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
