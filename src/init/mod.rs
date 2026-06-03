use std::path::PathBuf;

use crate::config::DEFAULT_CONFIG_TOML;
use crate::console::Console;
use crate::error::PeerError;
use crate::git::run_git;

/// Return .peer path
pub async fn handler(console: Console) -> Result<PathBuf, PeerError> {
    let cwd = std::env::current_dir().map_err(|e| PeerError::InvalidConfig {
        message: "cannot determine current directory".into(),
        source: Some(Box::new(e)),
    })?;

    run_git(&["--version"], &cwd, console).await?;

    if !cwd.join(".git").exists() {
        return Err(PeerError::InvalidConfig {
            message: "not a git repository (no .git/ found in current directory)".into(),
            source: None,
        });
    }

    let peer_dir = cwd.join(".peer");
    if peer_dir.exists() {
        return Err(PeerError::InvalidConfig {
            message: ".peer/ already exists".into(),
            source: None,
        });
    }

    std::fs::create_dir(&peer_dir).map_err(|e| PeerError::Internal {
        source: Box::new(e),
        message: format!("cannot create {}", peer_dir.display()),
    })?;

    let config_toml = peer_dir.join("config.toml");
    std::fs::write(&config_toml, DEFAULT_CONFIG_TOML).map_err(|e| PeerError::InvalidConfig {
        message: format!("failed to write {}", config_toml.display()),
        source: Some(Box::new(e)),
    })?;

    Ok(cwd)
}
