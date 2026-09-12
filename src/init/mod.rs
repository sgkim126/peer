use std::path::PathBuf;

use toml_edit::visit_mut::{self, VisitMut};
use toml_edit::{Decor, DocumentMut, Item, KeyMut, Table, Value};

use crate::config::DEFAULT_CONFIG_TOML;
use crate::error::PeerError;
use crate::git::run_git;

const PEER_GITIGNORE: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/resources/gitignore"));

/// Initialise the `.peer/` directory in the current git repository root.
/// Returns the current working directory on success.
pub async fn handler(
    provider: Option<String>,
    model: Option<String>,
    repo: Option<String>,
) -> Result<PathBuf, PeerError> {
    let cwd = std::env::current_dir().map_err(|e| PeerError::InvalidConfig {
        message: "cannot determine current directory".into(),
        source: Some(Box::new(e)),
    })?;

    run_git(&["--version"], &cwd).await?;

    if !cwd.join(".git").exists() {
        return Err(PeerError::InvalidConfig {
            message: "not a git repository (no .git/ found in current directory)".into(),
            source: None,
        });
    }

    let config = render_config(
        DEFAULT_CONFIG_TOML,
        provider.as_deref(),
        model.as_deref(),
        repo.as_deref(),
    )?;
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
    std::fs::write(&config_toml, config).map_err(|e| PeerError::Internal {
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

fn render_config(
    template: &str,
    provider: Option<&str>,
    model: Option<&str>,
    repo: Option<&str>,
) -> Result<String, PeerError> {
    let mut config = template
        .parse::<DocumentMut>()
        .map_err(|source| PeerError::Internal {
            message: "invalid default config template".into(),
            source: Box::new(source),
        })?;

    for (section, key, value) in [
        ("llm", "default_provider", provider),
        ("llm", "default_model", model),
        ("github", "repo", repo),
    ] {
        if let Some(value) = value {
            let item = &mut config[section][key];
            let mut replacement = Value::from(value);
            if let Some(existing) = item.as_value() {
                *replacement.decor_mut() = existing.decor().clone();
            }
            *item = Item::Value(replacement);
        }
    }

    if repo.is_some() {
        RemoveRepoExample.visit_document_mut(&mut config);
    }

    Ok(config.to_string())
}

struct RemoveRepoExample;

impl RemoveRepoExample {
    fn from_comments(comments: &str) -> String {
        comments
            .split_inclusive('\n')
            .filter(|line| line.trim() != "# repo = \"owner/repository\" # Set it to use --github.")
            .collect()
    }

    fn from_prefix(decor: &mut Decor) {
        if let Some(prefix) = decor.prefix().and_then(|prefix| prefix.as_str()) {
            decor.set_prefix(Self::from_comments(prefix));
        }
    }
}

// Standalone comments attach to the next table/key or to the document's trailing text.
// Only edit this metadata; matching text inside a string value must stay intact.
impl VisitMut for RemoveRepoExample {
    fn visit_document_mut(&mut self, document: &mut DocumentMut) {
        if let Some(trailing) = document.trailing().as_str() {
            document.set_trailing(Self::from_comments(trailing));
        }
        visit_mut::visit_document_mut(self, document);
    }

    fn visit_table_mut(&mut self, table: &mut Table) {
        Self::from_prefix(table.decor_mut());
        visit_mut::visit_table_mut(self, table);
    }

    fn visit_table_like_kv_mut(&mut self, mut key: KeyMut<'_>, item: &mut Item) {
        Self::from_prefix(key.leaf_decor_mut());
        visit_mut::visit_table_like_kv_mut(self, key, item);
    }
}

#[cfg(test)]
mod tests;
