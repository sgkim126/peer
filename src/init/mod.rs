use std::path::PathBuf;

use crate::config::DEFAULT_CONFIG_TOML;
use crate::console::Console;
use crate::error::PeerError;
use crate::git::run_git;

const PEER_GITIGNORE: &str = "cache/\n";

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

    write_peer_files(&peer_dir)?;

    Ok(cwd)
}

fn write_peer_files(peer_dir: &std::path::Path) -> Result<(), PeerError> {
    let config_toml = peer_dir.join("config.toml");
    std::fs::write(&config_toml, DEFAULT_CONFIG_TOML).map_err(|e| PeerError::InvalidConfig {
        message: format!("failed to write {}", config_toml.display()),
        source: Some(Box::new(e)),
    })?;

    let gitignore = peer_dir.join(".gitignore");
    std::fs::write(&gitignore, PEER_GITIGNORE).map_err(|e| PeerError::InvalidConfig {
        message: format!("failed to write {}", gitignore.display()),
        source: Some(Box::new(e)),
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_peer_files_ignores_cache_directory() {
        let directory = tempfile::tempdir().unwrap();
        let peer_dir = directory.path().join(".peer");
        std::fs::create_dir(&peer_dir).unwrap();

        write_peer_files(&peer_dir).unwrap();

        let gitignore = std::fs::read_to_string(peer_dir.join(".gitignore")).unwrap();
        assert_eq!(gitignore, "cache/\n");
    }
}
