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
            Self::Spawn(e) => write!(f, "failed to spawn git: {e}"),
            Self::NonZeroExit { status, stderr } => {
                write!(f, "git exited with status {status}: {stderr}")
            }
            Self::FromUtf8(e) => write!(f, "git output is not valid UTF-8: {e}"),
        }
    }
}

impl std::error::Error for GitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Spawn(e) => Some(e),
            Self::NonZeroExit { .. } => None,
            Self::FromUtf8(e) => Some(e),
        }
    }
}
