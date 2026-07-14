use std::fmt;

#[derive(Debug)]
pub enum GitError {
    Spawn(std::io::Error),
    NonZeroExit {
        status: i32,
        stderr: String,
    },
    FromUtf8(std::string::FromUtf8Error),
    InvalidCommitHash {
        value: String,
        reason: InvalidCommitHashReason,
    },
    InvalidRevision(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidCommitHashReason {
    TooShort,
    TooLong,
    InvalidCharacter,
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(e) => write!(f, "failed to spawn git: {e}"),
            Self::NonZeroExit { status, stderr } => {
                write!(f, "git exited with status {status}: {stderr}")
            }
            Self::FromUtf8(e) => write!(f, "git output is not valid UTF-8: {e}"),
            Self::InvalidCommitHash { value, reason } => {
                let reason = match reason {
                    InvalidCommitHashReason::TooShort => "is too short",
                    InvalidCommitHashReason::TooLong => "is too long",
                    InvalidCommitHashReason::InvalidCharacter => {
                        "contains a character outside lowercase hexadecimal"
                    }
                };
                write!(f, "{value} is an invalid commit hash because it {reason}")
            }
            Self::InvalidRevision(value) => {
                write!(f, "{value} is an invalid revision")
            }
        }
    }
}

impl std::error::Error for GitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Spawn(e) => Some(e),
            Self::NonZeroExit { .. } => None,
            Self::FromUtf8(e) => Some(e),
            Self::InvalidCommitHash { .. } => None,
            Self::InvalidRevision(_) => None,
        }
    }
}
