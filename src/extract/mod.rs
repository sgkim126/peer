mod commit_message;
mod error;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::cli::ExtractCommand;
use crate::config::Config;
use crate::console::Console;

pub use self::commit_message::CommitMessage;
use self::error::ExtractError;

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
    CommitMessage(CommitMessage),
}

pub async fn handler(
    console: Console,
    command: &ExtractCommand,
    _config: Config,
    project_root: PathBuf,
) -> Result<ExtractData, ExtractError> {
    let extractor = Extractor::new(project_root, console);
    Ok(match command {
        ExtractCommand::CommitMessage { revision } => {
            ExtractData::CommitMessage(extractor.commit_message(revision).await?)
        }
        _ => unimplemented!(),
    })
}
