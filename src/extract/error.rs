use std::fmt;

use crate::git::GitError;

#[derive(Debug)]
pub enum ExtractError {
    Git(GitError),
    InvalidTwoDotRange(String),
    InvalidRevision(String),
    InvalidGrepSearchArguments(String),
    InvalidFileContentRange(String),
}

impl fmt::Display for ExtractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Git(source) => {
                write!(f, "cannot run git ({source})")
            }
            Self::InvalidTwoDotRange(range) => {
                write!(f, "{range} is not a two-dots range")
            }
            Self::InvalidRevision(rev) => {
                write!(f, "{rev} is not a valid revision")
            }
            Self::InvalidGrepSearchArguments(message) => {
                write!(f, "invalid arguments for grep_search: {message}")
            }
            Self::InvalidFileContentRange(message) => {
                write!(f, "invalid file content range: {message}")
            }
        }
    }
}

impl std::error::Error for ExtractError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Git(source) => Some(source),
            Self::InvalidTwoDotRange(_) => None,
            Self::InvalidRevision(_) => None,
            Self::InvalidGrepSearchArguments(_) => None,
            Self::InvalidFileContentRange(_) => None,
        }
    }
}

impl From<GitError> for ExtractError {
    fn from(err: GitError) -> Self {
        Self::Git(err)
    }
}
