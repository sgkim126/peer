use std::fmt;

use super::commands::CommandError;

#[derive(Debug)]
pub enum GitHubError {
    MissingRepository,
    InvalidRepository,
    Command(CommandError),
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
    InvalidPagination,
    IncompleteCommits,
    PullRequestChanged,
}

impl GitHubError {
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

impl fmt::Display for GitHubError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingRepository => {
                write!(
                    f,
                    "--github requires --repo <owner/name> or [github].repo in .peer/config.toml"
                )
            }
            Self::InvalidRepository => write!(f, "GitHub repository must use the form owner/name"),
            Self::Command(error) => error.fmt(f),
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
            Self::InvalidPagination => write!(f, "invalid GitHub pagination link"),
            Self::IncompleteCommits => write!(
                f,
                "GitHub pull request commit list is incomplete or changed while loading; retry with a stable PR containing at most 250 commits"
            ),
            Self::PullRequestChanged => write!(
                f,
                "GitHub pull request changed while loading comment positions; retry with a stable PR"
            ),
        }
    }
}

impl std::error::Error for GitHubError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::MissingRepository => None,
            Self::InvalidRepository => None,
            Self::Command(source) => Some(source),
            Self::Api { .. } => None,
            Self::Decode { source, .. } => Some(source),
            Self::InvalidPagination => None,
            Self::IncompleteCommits => None,
            Self::PullRequestChanged => None,
        }
    }
}

impl From<CommandError> for GitHubError {
    fn from(error: CommandError) -> Self {
        match error {
            CommandError::Api {
                endpoint,
                status,
                rate_limited,
                permission_denied,
            } => Self::Api {
                endpoint,
                status,
                rate_limited,
                permission_denied,
            },
            CommandError::Decode { endpoint, source } => Self::Decode { endpoint, source },
            CommandError::InvalidPagination => Self::InvalidPagination,
            error => Self::Command(error),
        }
    }
}
