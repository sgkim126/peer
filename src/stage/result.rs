use std::fmt;

use serde::{Deserialize, Serialize};

use crate::git::CommitHash;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct FileLocation {
    pub file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Finding {
    pub commit: CommitHash,
    pub severity: Severity,
    pub message: String,
    #[serde(flatten)]
    pub location: Option<FileLocation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum StageTarget {
    Commit(CommitHash),
    Range { from: CommitHash, to: CommitHash },
}

impl fmt::Display for StageTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Commit(commit) => commit.fmt(f),
            Self::Range { from, to } => write!(f, "{from}..{to}"),
        }
    }
}
