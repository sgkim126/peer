use serde::Serialize;

use super::CacheKeyError;

const BINARY_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, PartialEq)]
pub struct CacheKey {
    pub namespace: String,
    pub provider: String,
    pub model: String,
    pub params_hash: String,
}

impl CacheKey {
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

    pub fn version() -> String {
        cache_version(BINARY_VERSION)
    }
}

fn cache_version(version: &str) -> String {
    version.split('.').take(2).collect::<Vec<_>>().join(".")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CacheVersion {
    major: u64,
    minor: u64,
}

impl CacheVersion {
    pub fn parse(value: &str) -> Option<Self> {
        let mut parts = value.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self { major, minor })
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

    #[test]
    fn patch_version_does_not_change_cache_version() {
        assert_eq!(cache_version("1.2.3"), "1.2");
        assert_eq!(cache_version("1.2.99"), "1.2");
    }

    #[test]
    fn parses_and_orders_cache_versions_numerically() {
        let one_nine = CacheVersion::parse("1.9").unwrap();
        let one_ten = CacheVersion::parse("1.10").unwrap();

        assert!(one_nine < one_ten);
        assert_eq!(CacheVersion::parse("1.10"), Some(one_ten));
    }

    #[test]
    fn rejects_invalid_cache_versions() {
        for value in [
            "",
            "1",
            "1.",
            ".1",
            "1.2.3",
            "v1.2",
            "1.x",
            "18446744073709551616.0",
        ] {
            assert_eq!(CacheVersion::parse(value), None, "{value}");
        }
    }
}
