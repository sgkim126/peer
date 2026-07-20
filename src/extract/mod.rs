mod commit_diff;
mod commit_files;
mod commit_list;
mod commit_message;
mod error;
mod file_content;
mod file_diff;
mod grep;
mod list_tree;

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cli::ExtractCommand;
use crate::config::Config;
use crate::console::Console;

pub use self::commit_diff::CommitDiff;
pub use self::commit_files::CommitFiles;
pub use self::commit_list::CommitList;
pub use self::commit_message::CommitMessage;
pub use self::error::ExtractError;
pub use self::file_content::FileContent;
pub use self::file_diff::FileDiff;
pub use self::grep::GrepResult;
pub use self::list_tree::TreeListing;
#[expect(unused_imports)]
pub use self::list_tree::{TreeEntry, TreeEntryKind};

/// Provides the programmatic entry point to repository extraction.
pub struct Extractor {
    project_root: PathBuf,
    console: Console,
}

impl Extractor {
    pub fn new(project_root: PathBuf, console: Console) -> Self {
        Self {
            project_root,
            console,
        }
    }
}

#[derive(Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "command", rename_all = "kebab-case")]
pub enum ExtractData {
    CommitDiff(CommitDiff),
    CommitFiles(CommitFiles),
    CommitList(CommitList),
    CommitMessage(CommitMessage),
    FileContent(FileContent),
    FileDiff(FileDiff),
    Grep(GrepResult),
    ListTree(TreeListing),
}

pub async fn handler(
    console: Console,
    command: &ExtractCommand,
    _config: Config,
    project_root: PathBuf,
) -> Result<ExtractData, ExtractError> {
    let extractor = Extractor::new(project_root, console);
    Ok(match command {
        ExtractCommand::CommitDiff { revision } => {
            ExtractData::CommitDiff(extractor.commit_diff(revision).await?)
        }
        ExtractCommand::CommitFiles { revision } => {
            ExtractData::CommitFiles(extractor.commit_files(revision).await?)
        }
        ExtractCommand::CommitList { range } => {
            ExtractData::CommitList(extractor.commit_list(range).await?)
        }
        ExtractCommand::CommitMessage { revision } => {
            ExtractData::CommitMessage(extractor.commit_message(revision).await?)
        }
        ExtractCommand::FileContent { revision, path } => {
            ExtractData::FileContent(extractor.file_content(revision, path).await?)
        }
        ExtractCommand::FileDiff {
            from_revision,
            to_revision,
            path,
        } => ExtractData::FileDiff(
            extractor
                .file_diff(from_revision, to_revision, path)
                .await?,
        ),
        ExtractCommand::ListTree {
            revision,
            path,
            recursive,
        } => ExtractData::ListTree(
            extractor
                .list_tree(revision, path.as_deref(), *recursive)
                .await?,
        ),
        ExtractCommand::Grep {
            revision,
            query,
            path,
            context_lines,
        } => ExtractData::Grep(
            extractor
                .grep(query, revision, path.as_deref(), *context_lines)
                .await?,
        ),
    })
}

fn validate_repository_relative_path(path: &Path) -> Result<(), ExtractError> {
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
