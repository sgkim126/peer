use std::{env::VarError, fmt};

use super::commands::CommandError;

#[derive(Debug)]
pub enum GitLabError {
    MissingRepository,
    InvalidRepository,
    MissingToken,
    InvalidToken,
    Client(reqwest::Error),
    Command(CommandError),
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
    InvalidPagination,
    InvalidMergeRequest,
    MergeRequestNotReady,
    IncompleteCommits,
    MergeRequestChanged,
}

impl GitLabError {
    /// The server may have accepted a POST even though its response was lost.
    pub fn may_have_published(&self) -> bool {
        match self {
            Self::Command(error) => error.may_have_published(),
            Self::Decode { .. }
            | Self::Api {
                status: 408 | 500..=599,
                ..
            } => true,
            _ => false,
        }
    }
}

impl fmt::Display for GitLabError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingRepository => write!(
                f,
                "--gitlab requires --repo <namespace/project> or [gitlab].repo in .peer/config.toml"
            ),
            Self::InvalidRepository => write!(
                f,
                "GitLab repository must use the form namespace/project (subgroups are supported)"
            ),
            Self::MissingToken => write!(
                f,
                "GitLab access requires a non-empty GITLAB_TOKEN environment variable"
            ),
            Self::InvalidToken => write!(f, "GITLAB_TOKEN is not a valid HTTP private token"),
            Self::Client(_) => write!(f, "failed to configure the GitLab HTTP client"),
            Self::Command(error) => error.fmt(f),
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
                        403 => "access denied; check token permissions",
                        404 => "merge request or project not found or inaccessible",
                        _ => "API request failed",
                    }
                };
                write!(f, "GitLab {reason} (HTTP {status}): {endpoint}")
            }
            Self::Decode { endpoint, .. } => write!(f, "invalid GitLab response: {endpoint}"),
            Self::InvalidPagination => write!(f, "invalid GitLab pagination link"),
            Self::InvalidMergeRequest => {
                write!(f, "GitLab returned an inconsistent merge request identity")
            }
            Self::MergeRequestNotReady => write!(
                f,
                "GitLab merge request diff references are not ready; retry after the MR finishes updating"
            ),
            Self::IncompleteCommits => write!(
                f,
                "GitLab merge request commit list is incomplete or inconsistent; retry with a stable MR"
            ),
            Self::MergeRequestChanged => write!(
                f,
                "GitLab merge request changed while loading; retry with a stable MR"
            ),
        }
    }
}

impl std::error::Error for GitLabError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::MissingRepository => None,
            Self::InvalidRepository => None,
            Self::MissingToken => None,
            Self::InvalidToken => None,
            Self::Client(source) => Some(source),
            Self::Command(source) => Some(source),
            Self::Api { .. } => None,
            Self::Decode { source, .. } => Some(source),
            Self::InvalidPagination => None,
            Self::InvalidMergeRequest => None,
            Self::MergeRequestNotReady => None,
            Self::IncompleteCommits => None,
            Self::MergeRequestChanged => None,
        }
    }
}

impl From<CommandError> for GitLabError {
    fn from(error: CommandError) -> Self {
        match error {
            CommandError::Api {
                endpoint,
                status,
                rate_limited,
                position_invalid,
                commit_invalid,
            } => Self::Api {
                endpoint,
                status,
                rate_limited,
                position_invalid,
                commit_invalid,
            },
            CommandError::Decode { endpoint, source } => Self::Decode { endpoint, source },
            CommandError::InvalidPagination => Self::InvalidPagination,
            error => Self::Command(error),
        }
    }
}

impl From<VarError> for GitLabError {
    fn from(error: VarError) -> Self {
        match error {
            VarError::NotPresent => Self::MissingToken,
            VarError::NotUnicode(_) => Self::InvalidToken,
        }
    }
}

impl From<reqwest::Error> for GitLabError {
    fn from(error: reqwest::Error) -> Self {
        Self::Client(error)
    }
}
