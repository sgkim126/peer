use std::fmt;

use crate::git::GitError;

#[derive(Debug)]
pub enum PeerError {
    Internal {
        message: String,
        source: Box<dyn std::error::Error>,
    },
    InvalidConfig {
        message: String,
        source: Option<Box<dyn std::error::Error>>,
    },
    Git(GitError),
}

impl fmt::Display for PeerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Internal { message, source } => {
                write!(f, "{message}: ({source})")
            }
            Self::InvalidConfig { message, source } => {
                if let Some(source) = source {
                    write!(f, "{message} ({source})")
                } else {
                    write!(f, "{message}")
                }
            }
            Self::Git(source) => {
                write!(f, "cannot run git ({source})")
            }
        }
    }
}

impl std::error::Error for PeerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Internal { source, .. } => Some(source.as_ref()),
            Self::InvalidConfig { source, .. } => source.as_deref(),
            Self::Git(source) => Some(source),
        }
    }
}

impl From<GitError> for PeerError {
    fn from(err: GitError) -> Self {
        Self::Git(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::error::Error;

    #[test]
    fn config_error_source_chain_is_preserved() {
        let e = PeerError::InvalidConfig {
            message: "invalid config".into(),
            source: Some(Box::new(std::io::Error::other("underlying cause"))),
        };
        assert!(e.source().is_some());
    }

    #[test]
    fn config_error_without_source_has_no_chain() {
        let e = PeerError::InvalidConfig {
            message: "not found".into(),
            source: None,
        };

        assert!(e.source().is_none());
    }
}
