use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::PeerError;

const SUPPORTED_VERSIONS: [u32; 1] = [1];

#[derive(Debug, PartialEq, Deserialize)]
#[allow(dead_code)]
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

/// Walks parent directories from `from` looking for `.peer/config.toml`.
/// Returns the parsed config and the project root (the directory containing `.peer/`).
#[allow(dead_code)]
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
    fn fails_when_config_not_found() {
        let tmp = tempfile::tempdir().unwrap();
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
}
