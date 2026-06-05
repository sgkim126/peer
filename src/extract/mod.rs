mod commit_diff;
mod commit_files;
mod commit_list;
mod commit_message;
mod error;
mod file_content;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use self::error::ExtractError;
use crate::cli::ExtractCommand;
use crate::config::Config;
use crate::console::Console;

#[derive(Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "command", rename_all = "kebab-case")]
pub enum ExtractData {
    CommitDiff(commit_diff::CommitDiff),
    CommitFiles(commit_files::CommitFiles),
    CommitList(commit_list::CommitList),
    CommitMessage(commit_message::CommitMessage),
    FileContent(file_content::FileContent),
}

pub async fn handler(
    console: Console,
    command: &ExtractCommand,
    _config: Config,
    project_root: PathBuf,
) -> Result<ExtractData, ExtractError> {
    Ok(match command {
        ExtractCommand::CommitDiff { revision } => ExtractData::CommitDiff(
            commit_diff::commit_diff(revision, &project_root, console).await?,
        ),
        ExtractCommand::CommitFiles { revision } => ExtractData::CommitFiles(
            commit_files::commit_files(revision, &project_root, console).await?,
        ),
        ExtractCommand::CommitList { range } => {
            ExtractData::CommitList(commit_list::commit_list(range, &project_root, console).await?)
        }
        ExtractCommand::CommitMessage { revision } => ExtractData::CommitMessage(
            commit_message::commit_message(revision, &project_root, console).await?,
        ),
        ExtractCommand::FileContent { path, revision } => ExtractData::FileContent(
            file_content::file_content(Path::new(path), revision, &project_root, console).await?,
        ),
    })
}
