use std::{
    collections::HashSet,
    num::NonZeroU32,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::error::PeerError;

const SUPPORTED_VERSIONS: [u32; 1] = [1];

#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub struct Config {
    pub version: u32,
    pub review: ReviewConfig,
    pub llm: LlmConfig,
    pub providers: Vec<ProviderConfig>,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewConfig {
    pub max_commits: NonZeroU32,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LlmConfig {
    pub default_provider: String,
    pub default_model: String,
    pub confidence_threshold: f64,
    pub max_iterations: NonZeroU32,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    pub name: String,
    pub api_key_env: String,
    pub base_url: Option<String>,
    pub models: Vec<ModelConfig>,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelConfig {
    pub name: String,
    pub input_per_1m_usd: f64,
    pub output_per_1m_usd: f64,
}

/// Walks parent directories from `from` looking for `.peer/config.toml`.
/// Returns the parsed config and the project root (the directory containing `.peer/`).
#[allow(dead_code)]
pub fn discover(from: &Path) -> Result<(Config, PathBuf), PeerError> {
    if !from.is_absolute() {
        return Err(PeerError::invalid_config(format!(
            "config discovery path must be absolute: {}",
            from.display()
        )));
    }

    for dir in from.ancestors() {
        let config_path = dir.join(".peer").join("config.toml");
        match std::fs::read_to_string(&config_path) {
            Ok(content) => return parse_and_validate(&content, dir.to_path_buf(), &config_path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(PeerError::InvalidConfig {
                    message: format!("cannot read {}", config_path.display()),
                    source: Some(Box::new(error)),
                });
            }
        }
    }
    Err(PeerError::invalid_config(format!(
        "no .peer/config.toml found from {}",
        from.display()
    )))
}

fn parse_and_validate(
    content: &str,
    project_root: PathBuf,
    config_path: &Path,
) -> Result<(Config, PathBuf), PeerError> {
    let config: Config = toml::from_str(content).map_err(|e| PeerError::InvalidConfig {
        message: format!("invalid config in {}", config_path.display()),
        source: Some(Box::new(e)),
    })?;
    if !SUPPORTED_VERSIONS.contains(&config.version) {
        return Err(PeerError::invalid_config(format!(
            "unsupported config version {} (expected {SUPPORTED_VERSIONS:?})",
            config.version
        )));
    }

    if config.providers.is_empty() {
        return Err(PeerError::invalid_config(
            "at least one provider must be configured",
        ));
    }

    let mut provider_names = HashSet::new();
    for provider in &config.providers {
        if !provider_names.insert(&provider.name) {
            return Err(PeerError::invalid_config(format!(
                "provider name '{}' is configured more than once",
                provider.name
            )));
        }
    }

    if let Some(provider) = config
        .providers
        .iter()
        .find(|provider| provider.models.is_empty())
    {
        return Err(PeerError::invalid_config(format!(
            "provider '{}' must configure at least one model",
            provider.name
        )));
    }

    for provider in &config.providers {
        let mut model_names = HashSet::new();
        for model in &provider.models {
            if !model_names.insert(&model.name) {
                return Err(PeerError::invalid_config(format!(
                    "provider '{}' configures model '{}' more than once",
                    provider.name, model.name
                )));
            }
        }
    }

    let default_provider = config
        .providers
        .iter()
        .find(|provider| provider.name == config.llm.default_provider)
        .ok_or_else(|| {
            PeerError::invalid_config(format!(
                "default provider '{}' is not configured",
                config.llm.default_provider
            ))
        })?;

    if !default_provider
        .models
        .iter()
        .any(|model| model.name == config.llm.default_model)
    {
        return Err(PeerError::invalid_config(format!(
            "default model '{}' is not configured for provider '{}'",
            config.llm.default_model, config.llm.default_provider
        )));
    }

    Ok((config, project_root))
}

/// the default `.peer/config.toml` content written by `peer init`.
pub const DEFAULT_CONFIG_TOML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/resources/default_config.toml"
));

#[cfg(test)]
mod tests {
    use super::*;
    use std::assert_matches;
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
        assert_matches!(discover(tmp.path()), Err(PeerError::InvalidConfig { .. }));
    }

    #[test]
    fn fails_on_version_mismatch() {
        let tmp = init_dir(&DEFAULT_CONFIG_TOML.replace("version = 1", "version = 99"));
        assert_matches!(discover(tmp.path()), Err(PeerError::InvalidConfig { .. }));
    }

    #[test]
    fn fails_on_invalid_toml() {
        let tmp = init_dir("[[[");
        let error = discover(tmp.path()).unwrap_err();

        assert!(error.to_string().starts_with(&format!(
            "invalid config in {}",
            tmp.path().join(".peer/config.toml").display()
        )));
    }

    #[test]
    fn fails_on_unknown_config_field() {
        let tmp = init_dir("unexpected = true");

        assert_matches!(discover(tmp.path()), Err(PeerError::InvalidConfig { .. }));
    }

    #[test]
    fn fails_when_max_commits_is_zero() {
        let tmp = init_dir(&DEFAULT_CONFIG_TOML.replace("max_commits = 10", "max_commits = 0"));

        assert_matches!(discover(tmp.path()), Err(PeerError::InvalidConfig { .. }));
    }

    #[test]
    fn fails_when_max_iterations_is_zero() {
        let tmp =
            init_dir(&DEFAULT_CONFIG_TOML.replace("max_iterations = 5", "max_iterations = 0"));

        assert_matches!(discover(tmp.path()), Err(PeerError::InvalidConfig { .. }));
    }

    #[test]
    fn fails_when_config_path_is_a_directory() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join(".peer/config.toml")).unwrap();

        assert_matches!(discover(tmp.path()), Err(PeerError::InvalidConfig { .. }));
    }

    #[test]
    fn fails_when_no_providers_are_configured() {
        let tmp = init_dir(
            r#"version = 1

providers = []

[review]
max_commits = 10

[llm]
default_provider = "mistral"
default_model = "mistral-large-latest"
confidence_threshold = 0.8
max_iterations = 5
"#,
        );

        let error = discover(tmp.path()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "at least one provider must be configured"
        );
    }

    #[test]
    fn fails_when_a_provider_has_no_models() {
        let tmp = init_dir(
            r#"version = 1

[review]
max_commits = 10

[llm]
default_provider = "mistral"
default_model = "mistral-large-latest"
confidence_threshold = 0.8
max_iterations = 5

[[providers]]
name = "mistral"
api_key_env = "MISTRAL_API_KEY"
models = []
"#,
        );

        let error = discover(tmp.path()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "provider 'mistral' must configure at least one model"
        );
    }

    #[test]
    fn fails_when_provider_names_are_duplicated() {
        let tmp = init_dir(&format!(
            r#"{DEFAULT_CONFIG_TOML}

[[providers]]
name = "mistral"
api_key_env = "SECOND_MISTRAL_API_KEY"
models = [{{ name = "another-model", input_per_1m_usd = 1.0, output_per_1m_usd = 1.0 }}]
"#
        ));

        let error = discover(tmp.path()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "provider name 'mistral' is configured more than once"
        );
    }

    #[test]
    fn fails_when_model_names_are_duplicated_within_a_provider() {
        let tmp = init_dir(&format!(
            r#"{DEFAULT_CONFIG_TOML}

[[providers]]
name = "duplicate-model-test"
api_key_env = "DUPLICATE_MODEL_TEST_API_KEY"
models = [
    {{ name = "same-model", input_per_1m_usd = 1.0, output_per_1m_usd = 2.0 }},
    {{ name = "same-model", input_per_1m_usd = 3.0, output_per_1m_usd = 4.0 }},
]
"#
        ));

        let error = discover(tmp.path()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "provider 'duplicate-model-test' configures model 'same-model' more than once"
        );
    }

    #[test]
    fn fails_when_default_provider_is_not_configured() {
        let tmp = init_dir(&DEFAULT_CONFIG_TOML.replace(
            "default_provider = \"mistral\"",
            "default_provider = \"missing\"",
        ));

        assert_matches!(discover(tmp.path()), Err(PeerError::InvalidConfig { .. }));
    }

    #[test]
    fn fails_when_default_model_is_not_configured_for_default_provider() {
        let tmp = init_dir(&DEFAULT_CONFIG_TOML.replace(
            "default_model = \"mistral-large-2512\"",
            "default_model = \"missing\"",
        ));

        assert_matches!(discover(tmp.path()), Err(PeerError::InvalidConfig { .. }));
    }

    #[test]
    fn default_config_parses_correctly() {
        let config: Config = toml::from_str(DEFAULT_CONFIG_TOML).unwrap();

        assert_eq!(
            config,
            Config {
                version: 1,
                review: ReviewConfig {
                    max_commits: NonZeroU32::new(10).unwrap(),
                },
                llm: LlmConfig {
                    default_provider: "mistral".into(),
                    default_model: "mistral-large-2512".into(),
                    confidence_threshold: 0.8,
                    max_iterations: NonZeroU32::new(5).unwrap(),
                },
                providers: vec![
                    ProviderConfig {
                        name: "mistral".into(),
                        api_key_env: "MISTRAL_API_KEY".into(),
                        base_url: None,
                        models: vec![
                            ModelConfig {
                                name: "mistral-large-2512".into(),
                                input_per_1m_usd: 0.5,
                                output_per_1m_usd: 1.5,
                            },
                            ModelConfig {
                                name: "mistral-medium-3-5".into(),
                                input_per_1m_usd: 1.5,
                                output_per_1m_usd: 7.5,
                            },
                            ModelConfig {
                                name: "mistral-small-2603".into(),
                                input_per_1m_usd: 0.15,
                                output_per_1m_usd: 0.6,
                            },
                        ],
                    },
                    ProviderConfig {
                        name: "openai".into(),
                        api_key_env: "OPENAI_API_KEY".into(),
                        base_url: None,
                        models: vec![
                            ModelConfig {
                                name: "gpt-5.5".into(),
                                input_per_1m_usd: 5.0,
                                output_per_1m_usd: 30.0,
                            },
                            ModelConfig {
                                name: "gpt-5.4".into(),
                                input_per_1m_usd: 2.5,
                                output_per_1m_usd: 15.0,
                            },
                            ModelConfig {
                                name: "gpt-5.4-mini".into(),
                                input_per_1m_usd: 0.75,
                                output_per_1m_usd: 4.5,
                            },
                        ],
                    },
                    ProviderConfig {
                        name: "anthropic".into(),
                        api_key_env: "ANTHROPIC_API_KEY".into(),
                        base_url: None,
                        models: vec![
                            ModelConfig {
                                name: "claude-sonnet-5".into(),
                                input_per_1m_usd: 2.0,
                                output_per_1m_usd: 10.0,
                            },
                            ModelConfig {
                                name: "claude-opus-4-8".into(),
                                input_per_1m_usd: 5.0,
                                output_per_1m_usd: 25.0,
                            },
                        ],
                    },
                    ProviderConfig {
                        name: "gemini".into(),
                        api_key_env: "GEMINI_API_KEY".into(),
                        base_url: None,
                        models: vec![
                            ModelConfig {
                                name: "gemini-3.5-flash".into(),
                                input_per_1m_usd: 1.5,
                                output_per_1m_usd: 9.0,
                            },
                            ModelConfig {
                                name: "gemini-3.1-pro-preview".into(),
                                input_per_1m_usd: 2.0,
                                output_per_1m_usd: 12.0,
                            },
                        ],
                    }
                ],
            }
        );
    }
}
