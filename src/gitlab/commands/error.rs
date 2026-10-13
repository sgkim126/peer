use std::fmt;

use crate::glab::GlabRunError;

pub enum CommandError {
    Run(GlabRunError),
    Api {
        endpoint: String,
        status: u16,
        rate_limited: bool,
        position_invalid: bool,
        commit_invalid: bool,
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

impl fmt::Debug for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Run(error) => f.debug_tuple("Run").field(error).finish(),
            Self::Api {
                endpoint,
                status,
                rate_limited,
                position_invalid,
                commit_invalid,
            } => f
                .debug_struct("Api")
                .field("endpoint", endpoint)
                .field("status", status)
                .field("rate_limited", rate_limited)
                .field("position_invalid", position_invalid)
                .field("commit_invalid", commit_invalid)
                .finish(),
            Self::Decode { endpoint, source } => f
                .debug_struct("Decode")
                .field("endpoint", endpoint)
                .field("source", source)
                .finish(),
            Self::InvalidResponse { command } => f
                .debug_struct("InvalidResponse")
                .field("command", command)
                .finish(),
            Self::InvalidPagination => f.write_str("InvalidPagination"),
            Self::Serialize(error) => f.debug_tuple("Serialize").field(error).finish(),
        }
    }
}

impl CommandError {
    #[cfg_attr(not(test), expect(dead_code))]
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
            Self::Run(error) => write!(f, "GitLab CLI failed: {error}"),
            Self::Api {
                endpoint,
                status,
                rate_limited,
                ..
            } => {
                let reason = if *rate_limited {
                    "API rate limit exceeded"
                } else {
                    match status {
                        401 => "authentication failed; check glab auth status",
                        403 => "API request forbidden",
                        404 => "merge request or project not found or inaccessible",
                        _ => "API request failed",
                    }
                };
                write!(f, "GitLab {reason} (HTTP {status}): {endpoint}")
            }
            Self::Decode { endpoint, .. } => write!(f, "invalid GitLab response: {endpoint}"),
            Self::InvalidResponse { command } => write!(f, "invalid output from {command}"),
            Self::InvalidPagination => write!(f, "invalid GitLab pagination link"),
            Self::Serialize(_) => write!(f, "failed to encode GitLab command input"),
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

impl From<GlabRunError> for CommandError {
    fn from(error: GlabRunError) -> Self {
        Self::Run(error)
    }
}
