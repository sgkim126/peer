use std::fmt;

#[derive(Debug)]
pub enum GitHubError {
    InvalidRepository,
}

impl fmt::Display for GitHubError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRepository => write!(f, "GitHub repository must use the form owner/name"),
        }
    }
}

impl std::error::Error for GitHubError {}
