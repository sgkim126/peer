use std::path::{Component, Path};

use serde::{Deserialize, Serialize};

use super::{ExtractError, Extractor};
use crate::git::{CommitHash, run_git};

const MAX_TREE_ENTRIES: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TreeEntryKind {
    File,
    Directory,
    Submodule,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct TreeEntry {
    pub path: String,
    pub kind: TreeEntryKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct TreeListing {
    pub entries: Vec<TreeEntry>,
    pub truncated: bool,
}

impl Extractor {
    pub async fn list_tree(
        &self,
        revision: &str,
        path: Option<&Path>,
        recursive: bool,
    ) -> Result<TreeListing, ExtractError> {
        validate_tree_path(path)?;
        let hash = CommitHash::resolve(revision, &self.project_root, self.console).await?;
        let path_prefix = path.map(|path| path.to_string_lossy().into_owned());
        let treeish = path_prefix
            .as_deref()
            .filter(|path| !path.is_empty())
            .map_or_else(|| hash.to_string(), |path| format!("{hash}:{path}"));
        let args = if recursive {
            vec!["ls-tree", "-rz", "-t", &treeish]
        } else {
            vec!["ls-tree", "-z", &treeish]
        };
        let output = run_git(&args, &self.project_root, self.console).await?;

        Ok(parse_tree_listing(&output, path_prefix.as_deref()))
    }
}

fn validate_tree_path(path: Option<&Path>) -> Result<(), ExtractError> {
    let Some(path) = path else {
        return Ok(());
    };

    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(ExtractError::InvalidRepositoryRelativePath(
            path.to_path_buf(),
        ));
    }

    Ok(())
}

fn parse_tree_listing(output: &str, path_prefix: Option<&str>) -> TreeListing {
    let mut parsed_entries = output
        .split('\0')
        .filter_map(parse_tree_entry)
        .map(|mut entry| {
            if let Some(prefix) = path_prefix.filter(|prefix| !prefix.is_empty()) {
                entry.path = format!("{prefix}/{}", entry.path);
            }
            entry
        });
    let entries = parsed_entries.by_ref().take(MAX_TREE_ENTRIES).collect();

    TreeListing {
        entries,
        truncated: parsed_entries.next().is_some(),
    }
}

fn parse_tree_entry(record: &str) -> Option<TreeEntry> {
    let (metadata, path) = record.split_once('\t')?;
    let mut metadata = metadata.split_whitespace();
    let _mode = metadata.next()?;
    let object_type = metadata.next()?;
    let _object = metadata.next()?;
    let kind = match object_type {
        "blob" => TreeEntryKind::File,
        "tree" => TreeEntryKind::Directory,
        "commit" => TreeEntryKind::Submodule,
        _ => return None,
    };

    Some(TreeEntry {
        path: path.to_string(),
        kind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tree_entries_and_prefixes_paths() {
        let listing = parse_tree_listing("100644 blob abc123\tlib.rs\0", Some("src"));

        assert_eq!(
            listing.entries,
            vec![TreeEntry {
                path: "src/lib.rs".to_string(),
                kind: TreeEntryKind::File,
            }]
        );
        assert!(!listing.truncated);
    }

    #[test]
    fn rejects_absolute_and_parent_tree_paths() {
        assert!(matches!(
            validate_tree_path(Some(Path::new("/tmp"))),
            Err(ExtractError::InvalidRepositoryRelativePath(_))
        ));
        assert!(matches!(
            validate_tree_path(Some(Path::new("src/../secret"))),
            Err(ExtractError::InvalidRepositoryRelativePath(_))
        ));
    }
}
