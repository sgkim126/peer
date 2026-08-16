use std::path::PathBuf;

use crate::config::DEFAULT_CONFIG_TOML;
use crate::console::Console;
use crate::error::PeerError;
use crate::git::{GitError, run_git};

const PEER_GITIGNORE: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/resources/gitignore"));

/// Initialise the `.peer/` directory in the current git repository root.
/// Returns the current working directory on success.
pub async fn handler(console: Console) -> Result<PathBuf, PeerError> {
    let cwd = std::env::current_dir().map_err(|e| PeerError::InvalidConfig {
        message: "cannot determine current directory".into(),
        source: Some(Box::new(e)),
    })?;

    let repo_root = match run_git(&["rev-parse", "--show-toplevel"], &cwd, console).await {
        Ok(repo_root) => repo_root,
        Err(err @ GitError::NonZeroExit { .. }) => {
            match run_git(&["rev-parse", "--git-dir"], &cwd, console).await {
                Ok(_) => return Err(PeerError::Git(err)),
                Err(GitError::NonZeroExit { .. }) => {
                    return Err(PeerError::invalid_config("not in a git repository"));
                }
                Err(err) => return Err(PeerError::Git(err)),
            }
        }
        Err(err) => return Err(PeerError::Git(err)),
    };
    let repo_root = PathBuf::from(repo_root.trim_end_matches(['\r', '\n']));

    let canonical_cwd = std::fs::canonicalize(&cwd).map_err(|e| PeerError::Internal {
        message: format!("cannot resolve {}", cwd.display()),
        source: Box::new(e),
    })?;
    let canonical_repo_root =
        std::fs::canonicalize(&repo_root).map_err(|e| PeerError::Internal {
            message: format!("cannot resolve {}", repo_root.display()),
            source: Box::new(e),
        })?;

    if canonical_cwd != canonical_repo_root {
        return Err(PeerError::invalid_config(format!(
            "peer init must be run from the repository root; run it again from {}",
            repo_root.display()
        )));
    }

    let peer_dir = cwd.join(".peer");
    if let Err(e) = std::fs::create_dir(&peer_dir) {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            return Err(PeerError::InvalidConfig {
                message: ".peer/ already exists".into(),
                source: None,
            });
        }

        return Err(PeerError::Internal {
            source: Box::new(e),
            message: format!("cannot create {}", peer_dir.display()),
        });
    }

    let config_toml = peer_dir.join("config.toml");
    std::fs::write(&config_toml, DEFAULT_CONFIG_TOML).map_err(|e| PeerError::Internal {
        message: format!("failed to write {}", config_toml.display()),
        source: Box::new(e),
    })?;
    let gitignore = peer_dir.join(".gitignore");
    std::fs::write(&gitignore, PEER_GITIGNORE).map_err(|e| PeerError::Internal {
        message: format!("failed to write {}", gitignore.display()),
        source: Box::new(e),
    })?;

    Ok(cwd)
}
