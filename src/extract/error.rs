use std::fmt;

use crate::git::GitError;

#[derive(Debug)]
pub enum ExtractError {
    Git(GitError),
}

impl fmt::Display for ExtractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Git(source) => {
                write!(f, "cannot run git ({source})")
            }
        }
    }
}

impl std::error::Error for ExtractError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Git(source) => Some(source),
        }
    }
}

impl From<GitError> for ExtractError {
    fn from(err: GitError) -> Self {
        Self::Git(err)
    }
}
