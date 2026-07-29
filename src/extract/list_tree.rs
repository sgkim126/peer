use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::git::{CommitHash, run_git};

use super::{ExtractError, Extractor, validate_repository_relative_path};

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
        self.debug(format_args!(
            "extract list tree: {revision} path={path:?} recursive={recursive}"
        ));
        if let Some(path) = path {
            validate_repository_relative_path(path)?;
        }
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

        parse_tree_listing(&output, path_prefix.as_deref())
    }
}

fn parse_tree_listing(
    output: &str,
    path_prefix: Option<&str>,
) -> Result<TreeListing, ExtractError> {
    let mut entries = Vec::new();
    let mut truncated = false;

    for record in output.split_terminator('\0') {
        let (metadata, path) = record
            .split_once('\t')
            .ok_or(ExtractError::MalformedGitOutput(format!(
                "invalid ls-tree record: {record:?}"
            )))?;
        let mut metadata = metadata.split_whitespace();
        let _mode = metadata
            .next()
            .ok_or(ExtractError::MalformedGitOutput(format!(
                "invalid ls-tree record: {record:?}"
            )))?;
        let object_type = metadata
            .next()
            .ok_or(ExtractError::MalformedGitOutput(format!(
                "invalid ls-tree record: {record:?}"
            )))?;
        let _object = metadata
            .next()
            .ok_or(ExtractError::MalformedGitOutput(format!(
                "invalid ls-tree record: {record:?}"
            )))?;
        if metadata.next().is_some() || path.is_empty() {
            return Err(ExtractError::MalformedGitOutput(format!(
                "invalid ls-tree record: {record:?}"
            )));
        }
        let kind = match object_type {
            "blob" => TreeEntryKind::File,
            "tree" => TreeEntryKind::Directory,
            "commit" => TreeEntryKind::Submodule,
            _ => {
                return Err(ExtractError::MalformedGitOutput(format!(
                    "invalid ls-tree record: {record:?}"
                )));
            }
        };

        let mut entry = TreeEntry {
            path: path.to_string(),
            kind,
        };
        if let Some(prefix) = path_prefix.filter(|prefix| !prefix.is_empty()) {
            entry.path = format!("{prefix}/{}", entry.path);
        }
        if entries.len() < MAX_TREE_ENTRIES {
            entries.push(entry);
        } else {
            truncated = true;
        }
    }

    Ok(TreeListing { entries, truncated })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::assert_matches;

    #[test]
    fn parses_entries_and_prefixes_paths() {
        let listing = parse_tree_listing(
            concat!(
                "100644 blob abc123\tlib.rs\0",
                "040000 tree def456\tnested\0"
            ),
            Some("src"),
        )
        .unwrap();

        assert_eq!(
            listing.entries,
            vec![
                TreeEntry {
                    path: "src/lib.rs".to_string(),
                    kind: TreeEntryKind::File,
                },
                TreeEntry {
                    path: "src/nested".to_string(),
                    kind: TreeEntryKind::Directory,
                },
            ]
        );
        assert!(!listing.truncated);
    }

    #[test]
    fn parses_submodules() {
        let listing = parse_tree_listing("160000 commit abc123\tvendor/lib\0", None).unwrap();

        assert_eq!(listing.entries[0].kind, TreeEntryKind::Submodule);
    }

    #[test]
    fn rejects_malformed_records() {
        let error = parse_tree_listing("not-a-tree-record\0", None).unwrap_err();

        assert_matches!(error, ExtractError::MalformedGitOutput(_));
    }

    #[test]
    fn truncates_large_listings() {
        let output = (0..=MAX_TREE_ENTRIES)
            .map(|index| format!("100644 blob abc123\tfile-{index}.rs\0"))
            .collect::<String>();

        let listing = parse_tree_listing(&output, None).unwrap();

        assert_eq!(listing.entries.len(), MAX_TREE_ENTRIES);
        assert!(listing.truncated);
    }

    #[test]
    fn rejects_empty_path() {
        assert_matches!(
            validate_repository_relative_path(Path::new("")),
            Err(ExtractError::InvalidRepositoryRelativePath(_))
        );
    }

    #[test]
    fn rejects_absolute_path() {
        assert_matches!(
            validate_repository_relative_path(Path::new("/tmp")),
            Err(ExtractError::InvalidRepositoryRelativePath(_))
        );
    }

    #[test]
    fn rejects_parent_path() {
        assert_matches!(
            validate_repository_relative_path(Path::new("src/../secret")),
            Err(ExtractError::InvalidRepositoryRelativePath(_))
        );
    }
}
