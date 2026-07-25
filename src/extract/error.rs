use std::fmt;
use std::path::PathBuf;

use crate::git::GitError;

#[derive(Debug)]
pub enum ExtractError {
    Git(GitError),
    InvalidFileContentRange(String),
    InvalidGrepArguments(String),
    InvalidTwoDotRange(String),
    InvalidRepositoryRelativePath(PathBuf),
    MalformedGitOutput(String),
}

impl fmt::Display for ExtractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Git(source) => {
                write!(f, "cannot run git ({source})")
            }
            Self::InvalidFileContentRange(message) => {
                write!(f, "invalid file content range: {message}")
            }
            Self::InvalidGrepArguments(message) => {
                write!(f, "invalid grep search arguments: {message}")
            }
            Self::InvalidTwoDotRange(range) => {
                write!(f, "{range} is not a two-dot range")
            }
            Self::InvalidRepositoryRelativePath(path) => {
                write!(f, "{} is not a repository-relative path", path.display())
            }
            Self::MalformedGitOutput(message) => {
                write!(f, "git produced malformed output: {message}")
            }
        }
    }
}

impl std::error::Error for ExtractError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Git(source) => Some(source),
            Self::InvalidFileContentRange(_) => None,
            Self::InvalidGrepArguments(_) => None,
            Self::InvalidTwoDotRange(_) => None,
            Self::InvalidRepositoryRelativePath(_) => None,
            Self::MalformedGitOutput(_) => None,
        }
    }
}

impl From<GitError> for ExtractError {
    fn from(err: GitError) -> Self {
        Self::Git(err)
    }
}
