mod commit_diff;
mod commit_files;
mod commit_list;
mod commit_message;
mod error;
mod file_content;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub use self::commit_diff::CommitDiff;
pub use self::commit_files::CommitFiles;
pub use self::commit_list::CommitList;
pub use self::commit_message::CommitMessage;
pub use self::error::ExtractError;
pub use self::file_content::FileContent;
use crate::cli::ExtractCommand;
use crate::config::Config;
use crate::console::Console;
use crate::git::{CommitHash, GitError};

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

    pub async fn resolve_commit(&self, revision: &str) -> Result<CommitHash, GitError> {
        CommitHash::resolve(revision, &self.project_root, self.console).await
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
        ExtractCommand::FileContent { path, revision } => {
            ExtractData::FileContent(extractor.file_content(path, revision).await?)
        }
    })
}
