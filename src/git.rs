use std::fmt;

#[derive(Debug)]
#[allow(dead_code)]
pub enum GitError {
    Spawn(std::io::Error),
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GitError::Spawn(e) => write!(f, "failed to spawn git: {e}"),
        }
    }
}

impl std::error::Error for GitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            GitError::Spawn(e) => Some(e),
        }
    }
}
