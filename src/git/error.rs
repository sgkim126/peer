use std::fmt;

#[derive(Debug)]
pub enum GitError {
    Spawn(std::io::Error),
    NonZeroExit { status: i32, stderr: String },
    FromUtf8(std::string::FromUtf8Error),
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GitError::Spawn(e) => write!(f, "failed to spawn git: {e}"),
            GitError::NonZeroExit { status, stderr } => {
                write!(f, "git exited with status {status}: {stderr}")
            }
            GitError::FromUtf8(e) => write!(f, "git output is not valid UTF-8: {e}"),
        }
    }
}

impl std::error::Error for GitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            GitError::Spawn(e) => Some(e),
            GitError::NonZeroExit { .. } => None,
            GitError::FromUtf8(e) => Some(e),
        }
    }
}
