use std::fs;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::PeerError;

const SUPPORTED_VERSIONS: [u32; 1] = [2];

#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    pub review: ReviewConfig,
    pub llm: LlmConfig,
    #[serde(default)]
    pub stages: StagesConfig,
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
    pub max_iterations: NonZeroU32,
}

/// Per-stage settings. Values omitted here fall back to `[llm]` settings.
#[derive(Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StagesConfig {
    #[serde(default)]
    pub review_context: StageConfig,
    #[serde(default)]
    pub knowledge: StageConfig,
    #[serde(default)]
    pub quality: StageConfig,
    #[serde(default)]
    pub security: StageConfig,
}

#[derive(Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StageConfig {
    pub max_iterations: Option<NonZeroU32>,
}

impl Config {
    /// Returns the configured iteration limit for a stage, falling back to `[llm]`.
    pub fn max_iterations_for(&self, stage: &str) -> NonZeroU32 {
        let override_value = match stage {
            "review_context" => self.stages.review_context.max_iterations,
            "knowledge" => self.stages.knowledge.max_iterations,
            "quality" => self.stages.quality.max_iterations,
            "security" => self.stages.security.max_iterations,
            _ => None,
        };

        override_value.unwrap_or(self.llm.max_iterations)
    }
}

/// Walks parent directories from `from` looking for `.peer/config.toml`.
/// Returns the parsed config and the project root (the directory containing `.peer/`).
pub fn discover(from: &Path) -> Result<(Config, PathBuf), PeerError> {
    let project_root = discover_peer_root(from)?;
    let config_path = project_root.join(".peer").join("config.toml");
    let content = fs::read_to_string(&config_path).map_err(|source| PeerError::InvalidConfig {
        message: format!("cannot read {}", config_path.display()),
        source: Some(Box::new(source)),
    })?;
    let config = parse_and_validate(&content, &config_path)?;
    Ok((config, project_root))
}

/// Walks parent directories from `from` looking for a `.peer/config.toml`
/// entry without reading or validating its contents.
pub fn discover_peer_root(from: &Path) -> Result<PathBuf, PeerError> {
    if !from.is_absolute() {
        return Err(PeerError::invalid_config(format!(
            "config discovery path must be absolute: {}",
            from.display()
        )));
    }

    for dir in from.ancestors() {
        let peer_path = dir.join(".peer");
        let peer_metadata = match fs::symlink_metadata(&peer_path) {
            Ok(metadata) => metadata,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(PeerError::InvalidConfig {
                    message: format!("cannot inspect {}", peer_path.display()),
                    source: Some(Box::new(source)),
                });
            }
        };
        if !peer_metadata.file_type().is_dir() {
            return Err(PeerError::invalid_config(format!(
                "{} is not a directory or is a symbolic link",
                peer_path.display()
            )));
        }

        let config_path = peer_path.join("config.toml");
        match fs::symlink_metadata(&config_path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(PeerError::invalid_config(format!(
                    "{} is a symbolic link",
                    config_path.display()
                )));
            }
            Ok(_) => return Ok(dir.to_path_buf()),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(PeerError::InvalidConfig {
                    message: format!("cannot inspect {}", config_path.display()),
                    source: Some(Box::new(source)),
                });
            }
        }
    }

    Err(PeerError::invalid_config(format!(
        "no .peer/config.toml found from {}",
        from.display()
    )))
}

fn parse_and_validate(content: &str, config_path: &Path) -> Result<Config, PeerError> {
    #[derive(Deserialize)]
    struct VersionProbe {
        version: u32,
    }

    let probe: VersionProbe =
        toml::from_str(content).map_err(|source| PeerError::InvalidConfig {
            message: format!("invalid config in {}", config_path.display()),
            source: Some(Box::new(source)),
        })?;
    if !SUPPORTED_VERSIONS.contains(&probe.version) {
        return Err(PeerError::invalid_config(format!(
            "unsupported config version {} (expected 2); run peer init again to create a v2 config",
            probe.version
        )));
    }
    toml::from_str(content).map_err(|source| PeerError::InvalidConfig {
        message: format!("invalid config in {}", config_path.display()),
        source: Some(Box::new(source)),
    })
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

        assert_eq!(config.version, 2);
        assert_eq!(root, tmp.path());
    }

    #[test]
    fn discovers_config_in_parent_dir() {
        let tmp = init_dir(DEFAULT_CONFIG_TOML);
        let subdir = tmp.path().join("sub/dir");
        fs::create_dir_all(&subdir).unwrap();

        assert_eq!(discover(&subdir).unwrap().1, tmp.path());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_symbolic_link_peer_directory() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let linked = init_dir(DEFAULT_CONFIG_TOML);
        symlink(linked.path().join(".peer"), tmp.path().join(".peer")).unwrap();

        assert!(
            discover(tmp.path())
                .unwrap_err()
                .to_string()
                .contains("symbolic link")
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_symbolic_link_config_file() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let peer = tmp.path().join(".peer");
        fs::create_dir(&peer).unwrap();
        let linked = init_dir(DEFAULT_CONFIG_TOML);
        symlink(
            linked.path().join(".peer/config.toml"),
            peer.join("config.toml"),
        )
        .unwrap();

        assert!(
            discover(tmp.path())
                .unwrap_err()
                .to_string()
                .contains("symbolic link")
        );
    }

    #[test]
    fn fails_when_config_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        assert_matches!(discover(tmp.path()), Err(PeerError::InvalidConfig { .. }));
    }

    #[test]
    fn rejects_v1_with_reinitialization_guidance() {
        let v1 = DEFAULT_CONFIG_TOML.replace("version = 2", "version = 1");
        let error = discover(init_dir(&v1).path()).unwrap_err();

        assert!(error.to_string().contains("run peer init again"));
    }

    #[test]
    fn fails_on_invalid_or_unknown_fields() {
        assert_matches!(
            discover(init_dir("[[[").path()),
            Err(PeerError::InvalidConfig { .. })
        );
        let unknown = format!("unexpected = true\n{DEFAULT_CONFIG_TOML}");
        assert_matches!(
            discover(init_dir(&unknown).path()),
            Err(PeerError::InvalidConfig { .. })
        );
    }

    #[test]
    fn nonzero_max_commits_are_required() {
        let commits = DEFAULT_CONFIG_TOML.replace("max_commits = 10", "max_commits = 0");
        assert_matches!(
            discover(init_dir(&commits).path()),
            Err(PeerError::InvalidConfig { .. })
        );
    }

    #[test]
    fn nonzero_max_iterations_are_required() {
        let turns = DEFAULT_CONFIG_TOML.replacen("max_iterations = 3", "max_iterations = 0", 1);
        assert_matches!(
            discover(init_dir(&turns).path()),
            Err(PeerError::InvalidConfig { .. })
        );
    }

    #[test]
    fn default_config_contains_only_pi_model_selection() {
        let config: Config = toml::from_str(DEFAULT_CONFIG_TOML).unwrap();

        assert_eq!(config.llm.default_provider, "mistral");
        assert_eq!(config.llm.default_model, "mistral-medium-3.5");
        assert_eq!(config.max_iterations_for("quality").get(), 10);
        assert_eq!(config.max_iterations_for("review_context").get(), 3);
        assert_eq!(config.max_iterations_for("knowledge").get(), 10);
    }

    #[test]
    fn removed_stage_overrides_are_rejected() {
        let error = toml::from_str::<Config>(
            r#"
version = 2
[review]
max_commits = 10
[llm]
default_provider = "mistral"
default_model = "model"
max_iterations = 3
[stages.commit_scope]
max_iterations = 4
"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("unknown field `commit_scope`"));
    }
}
