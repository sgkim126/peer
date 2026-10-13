use std::fmt;

use crate::gh::GhRunError;

#[derive(Debug)]
pub enum CommandError {
    Run(GhRunError),
    Api {
        endpoint: String,
        status: u16,
        rate_limited: bool,
        permission_denied: bool,
    },
    Decode {
        endpoint: String,
        source: serde_json::Error,
    },
    InvalidResponse {
        command: String,
    },
    InvalidPagination,
    Serialize(serde_json::Error),
}

impl CommandError {
    pub fn may_have_published(&self) -> bool {
        match self {
            Self::Run(error) => error.may_have_run(),
            Self::Api { status, .. } => matches!(status, 408 | 500..=599),
            Self::Decode { .. } => true,
            Self::InvalidResponse { .. } => true,
            Self::InvalidPagination => false,
            Self::Serialize(_) => false,
        }
    }
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Run(error) => write!(f, "GitHub CLI failed: {error}"),
            Self::Api {
                endpoint,
                status,
                rate_limited,
                permission_denied,
            } => {
                let reason = if *rate_limited {
                    "API rate limit exceeded"
                } else {
                    match status {
                        401 => "authentication failed; check gh auth status",
                        403 if *permission_denied => "access denied; check token permissions",
                        403 => "API request forbidden",
                        404 => "pull request or repository not found or inaccessible",
                        _ => "API request failed",
                    }
                };
                write!(f, "GitHub {reason} (HTTP {status}): {endpoint}")
            }
            Self::Decode { endpoint, .. } => write!(f, "invalid GitHub response: {endpoint}"),
            Self::InvalidResponse { command } => write!(f, "invalid output from {command}"),
            Self::InvalidPagination => write!(f, "invalid GitHub pagination link"),
            Self::Serialize(_) => write!(f, "failed to encode GitHub command input"),
        }
    }
}

impl std::error::Error for CommandError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Run(error) => Some(error),
            Self::Api { .. } => None,
            Self::Decode { source, .. } => Some(source),
            Self::InvalidResponse { .. } => None,
            Self::InvalidPagination => None,
            Self::Serialize(source) => Some(source),
        }
    }
}

impl From<GhRunError> for CommandError {
    fn from(error: GhRunError) -> Self {
        Self::Run(error)
    }
}
