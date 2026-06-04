mod commit_diff;
mod commit_files;
mod commit_message;
mod error;
mod file_content;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use self::error::ExtractError;
use crate::cli::ExtractCommand;
use crate::config::Config;
use crate::console::Console;
use crate::git::CommitHash;

#[derive(Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "command", rename_all = "kebab-case")]
pub enum ExtractData {
    CommitDiff(commit_diff::CommitDiff),
    CommitFiles(commit_files::CommitFiles),
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
        ExtractCommand::CommitDiff { revision } => {
            let hash = CommitHash::resolve(revision, &project_root, console).await?;
            ExtractData::CommitDiff(commit_diff::commit_diff(hash, &project_root, console).await?)
        }
        ExtractCommand::CommitFiles { revision } => {
            let hash = CommitHash::resolve(revision, &project_root, console).await?;
            ExtractData::CommitFiles(
                commit_files::commit_files(hash, &project_root, console).await?,
            )
        }
        ExtractCommand::CommitMessage { revision } => {
            let hash = CommitHash::resolve(revision, &project_root, console).await?;
            ExtractData::CommitMessage(
                commit_message::commit_message(hash, &project_root, console).await?,
            )
        }
        ExtractCommand::FileContent { path, revision } => {
            let hash = CommitHash::resolve(revision, &project_root, console).await?;
            ExtractData::FileContent(
                file_content::file_content(Path::new(path), hash, &project_root, console).await?,
            )
        }
        _ => unimplemented!(),
    })
}
