mod commit_files;
mod commit_message;
mod error;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub use self::error::ExtractError;
use crate::cli::ExtractCommand;
use crate::config::Config;
use crate::console::Console;
use crate::git::CommitHash;

#[derive(Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "command", rename_all = "kebab-case")]
pub enum ExtractData {
    CommitFiles(commit_files::CommitFiles),
    CommitMessage(commit_message::CommitMessage),
}

pub async fn handler(
    console: Console,
    command: &ExtractCommand,
    _config: Config,
    project_root: PathBuf,
) -> Result<ExtractData, ExtractError> {
    Ok(match command {
        ExtractCommand::CommitFiles { hash } => {
            let hash = CommitHash::new(hash).map_err(ExtractError::InvalidInput)?;
            ExtractData::CommitFiles(
                commit_files::commit_files(hash, &project_root, console).await?,
            )
        }
        ExtractCommand::CommitMessage { hash } => {
            let hash = CommitHash::new(hash).map_err(PeerError::InvalidInput)?;
            ExtractData::CommitMessage(
                commit_message::commit_message(hash, &project_root, console).await?,
            )
        }
        _ => unimplemented!(),
    })
}
