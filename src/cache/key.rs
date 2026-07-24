use serde::Serialize;

use super::error::CacheKeyError;

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(not(test), expect(dead_code))]
pub struct CacheKey {
    pub namespace: String,
    pub provider: String,
    pub model: String,
    pub params_hash: String,
}

impl CacheKey {
    #[cfg_attr(not(test), expect(dead_code))]
    pub fn from_params<T>(
        namespace: impl Into<String>,
        provider: impl Into<String>,
        model: impl Into<String>,
        params: &T,
    ) -> Result<Self, CacheKeyError>
    where
        T: Serialize,
    {
        let namespace = namespace.into();
        let provider = provider.into();
        let model = model.into();
        let bytes = serde_json::to_vec(params)?;
        Ok(Self {
            namespace,
            provider,
            model,
            params_hash: blake3::hash(&bytes).to_hex().to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_serializable_parameters_deterministically() {
        let first = CacheKey::from_params("check", "openai", "model", &serde_json::json!({"a": 1}))
            .unwrap();
        let second =
            CacheKey::from_params("check", "openai", "model", &serde_json::json!({"a": 1}))
                .unwrap();

        assert_eq!(first, second);
        assert_eq!(first.params_hash.len(), 64);
    }

    #[test]
    fn namespace_provider_and_model_do_not_change_params_hash() {
        let first = CacheKey::from_params("check", "provider/a", "model", &"params").unwrap();
        let second =
            CacheKey::from_params("other-check", "provider_a", "other-model", &"params").unwrap();

        assert_eq!(first.params_hash, second.params_hash);
    }
}
