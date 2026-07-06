use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::PeerError;

const SUPPORTED_VERSIONS: [u32; 1] = [1];

#[derive(Debug, PartialEq, Deserialize)]
pub struct Config {
    pub version: u32,
    pub review: ReviewConfig,
    pub llm: LlmConfig,
    pub providers: Vec<ProviderConfig>,
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct ReviewConfig {
    pub max_commits: u32,
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct LlmConfig {
    pub default_provider: String,
    pub default_model: String,
    pub confidence_threshold: f64,
    pub max_iterations: u32,
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct ProviderConfig {
    pub name: String,
    pub api_key_env: String,
    pub base_url: Option<String>,
    pub models: Vec<ModelConfig>,
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct ModelConfig {
    pub name: String,
    pub input_per_1m_usd: f64,
    pub output_per_1m_usd: f64,
}

impl Config {
    /// Finds the named provider and model, returning references to both.
    /// Returns `InvalidConfig` if either is absent.
    pub fn resolve_provider(
        &self,
        provider_name: &str,
        model_name: &str,
    ) -> Result<(&ProviderConfig, &ModelConfig), PeerError> {
        let provider = self
            .providers
            .iter()
            .find(|p| p.name == provider_name)
            .ok_or_else(|| PeerError::InvalidConfig {
                message: format!("provider '{provider_name}' not found in config"),
                source: None,
            })?;

        let model = provider
            .models
            .iter()
            .find(|m| m.name == model_name)
            .ok_or_else(|| PeerError::InvalidConfig {
                message: format!("model '{model_name}' not found in provider '{provider_name}'"),
                source: None,
            })?;

        Ok((provider, model))
    }
}

/// Walks parent directories from `from` looking for `.peer/config.toml`.
/// Returns the parsed config and the project root (the directory containing `.peer/`).
pub fn discover(from: &Path) -> Result<(Config, PathBuf), PeerError> {
    for dir in from.ancestors() {
        let config_path = dir.join(".peer").join("config.toml");
        if config_path.exists() {
            let content =
                std::fs::read_to_string(&config_path).map_err(|e| PeerError::InvalidConfig {
                    message: format!("cannot read {}", config_path.display()),
                    source: Some(Box::new(e)),
                })?;
            let config = parse_and_validate(&content)?;
            return Ok((config, dir.to_path_buf()));
        }
    }
    Err(PeerError::InvalidConfig {
        message: format!("no .peer/config.toml found from {}", from.display()),
        source: None,
    })
}

fn parse_and_validate(content: &str) -> Result<Config, PeerError> {
    let config: Config = toml::from_str(content).map_err(|e| PeerError::InvalidConfig {
        message: "invalid config".into(),
        source: Some(Box::new(e)),
    })?;
    if !SUPPORTED_VERSIONS.contains(&config.version) {
        return Err(PeerError::InvalidConfig {
            message: format!(
                "unsupported config version {} (expected {SUPPORTED_VERSIONS:?})",
                config.version
            ),
            source: None,
        });
    }
    Ok(config)
}

/// the default `.peer/config.toml` content written by `peer init`.
pub const DEFAULT_CONFIG_TOML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/resources/default_config.toml"
));

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[must_use]
    fn init_dir(content: &str) -> TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let peer_dir = dir.join(".peer");
        fs::create_dir_all(&peer_dir).unwrap();
        fs::write(peer_dir.join("config.toml"), content).unwrap();
        tmp
    }

    #[test]
    fn discovers_config_in_same_dir() {
        let tmp = init_dir(DEFAULT_CONFIG_TOML);
        let (config, root) = discover(tmp.path()).unwrap();

        assert_eq!(config.version, 1);
        assert_eq!(root, tmp.path());
    }

    #[test]
    fn discovers_config_in_parent_dir() {
        let tmp = init_dir(DEFAULT_CONFIG_TOML);
        let subdir = tmp.path().join("sub").join("dir");
        fs::create_dir_all(&subdir).unwrap();
        let (_, root) = discover(&subdir).unwrap();

        assert_eq!(root, tmp.path());
    }

    #[test]
    fn fails_when_config_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(matches!(
            discover(tmp.path()),
            Err(PeerError::InvalidConfig { .. })
        ));
    }

    #[test]
    fn fails_on_version_mismatch() {
        let tmp = init_dir(&DEFAULT_CONFIG_TOML.replace("version = 1", "version = 99"));
        assert!(matches!(
            discover(tmp.path()),
            Err(PeerError::InvalidConfig { .. })
        ));
    }

    #[test]
    fn fails_on_invalid_toml() {
        let tmp = init_dir("[[[");
        assert!(matches!(
            discover(tmp.path()),
            Err(PeerError::InvalidConfig { .. })
        ));
    }

    #[test]
    fn resolve_provider_returns_provider_and_model() {
        let config: Config = toml::from_str(DEFAULT_CONFIG_TOML).unwrap();
        let (provider, model) = config
            .resolve_provider("mistral", "mistral-large-latest")
            .unwrap();
        assert_eq!(provider.name, "mistral");
        assert_eq!(provider.api_key_env, "MISTRAL_API_KEY");
        assert_eq!(model.name, "mistral-large-latest");
        assert_eq!(model.input_per_1m_usd, 2.0);
        assert_eq!(model.output_per_1m_usd, 6.0);
    }

    #[test]
    fn resolve_provider_fails_when_provider_missing() {
        let config: Config = toml::from_str(DEFAULT_CONFIG_TOML).unwrap();
        assert!(matches!(
            config.resolve_provider("nonexistent", "mistral-large-latest"),
            Err(PeerError::InvalidConfig { source: None, .. })
        ));
    }

    #[test]
    fn resolve_provider_fails_when_model_missing() {
        let config: Config = toml::from_str(DEFAULT_CONFIG_TOML).unwrap();
        assert!(matches!(
            config.resolve_provider("mistral", "no-such-model"),
            Err(PeerError::InvalidConfig { source: None, .. })
        ));
    }

    #[test]
    fn default_config_parses_correctly() {
        let config: Config = toml::from_str(DEFAULT_CONFIG_TOML).unwrap();

        assert_eq!(
            config,
            Config {
                version: 1,
                review: ReviewConfig { max_commits: 10 },
                llm: LlmConfig {
                    default_provider: "mistral".into(),
                    default_model: "mistral-large-latest".into(),
                    confidence_threshold: 0.8,
                    max_iterations: 5,
                },
                providers: vec![
                    ProviderConfig {
                        name: "mistral".into(),
                        api_key_env: "MISTRAL_API_KEY".into(),
                        base_url: None,
                        models: vec![ModelConfig {
                            name: "mistral-large-latest".into(),
                            input_per_1m_usd: 2.0,
                            output_per_1m_usd: 6.0,
                        }],
                    },
                    ProviderConfig {
                        name: "openai".into(),
                        api_key_env: "OPENAI_API_KEY".into(),
                        base_url: None,
                        models: vec![ModelConfig {
                            name: "gpt-5.4-mini".into(),
                            input_per_1m_usd: 0.75,
                            output_per_1m_usd: 4.5,
                        }],
                    },
                    ProviderConfig {
                        name: "anthropic".into(),
                        api_key_env: "ANTHROPIC_API_KEY".into(),
                        base_url: None,
                        models: vec![ModelConfig {
                            name: "claude-sonnet-5".into(),
                            input_per_1m_usd: 2.0,
                            output_per_1m_usd: 10.0,
                        }],
                    },
                    ProviderConfig {
                        name: "gemini".into(),
                        api_key_env: "GEMINI_API_KEY".into(),
                        base_url: None,
                        models: vec![ModelConfig {
                            name: "gemini-3.5-flash".into(),
                            input_per_1m_usd: 1.5,
                            output_per_1m_usd: 9.0,
                        }],
                    }
                ],
            }
        );
    }
}
