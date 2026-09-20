use std::{env::VarError, fmt};

#[derive(Debug)]
#[expect(dead_code)]
pub enum GitLabError {
    MissingRepository,
    InvalidRepository,
    MissingToken,
    InvalidToken,
    Client(reqwest::Error),
    Request {
        endpoint: String,
        source: reqwest::Error,
    },
    Api {
        endpoint: String,
        status: u16,
        rate_limited: bool,
    },
    Decode {
        endpoint: String,
        source: reqwest::Error,
    },
    InvalidPagination,
}

impl GitLabError {
    /// The server may have accepted a POST even though its response was lost.
    #[cfg_attr(not(test), expect(dead_code))]
    pub fn may_have_published(&self) -> bool {
        matches!(
            self,
            Self::Request { .. }
                | Self::Decode { .. }
                | Self::Api {
                    status: 408 | 500..=599,
                    ..
                }
        )
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
            Self::Request { endpoint, source } => {
                let reason = if source.is_timeout() {
                    "timed out"
                } else {
                    "failed"
                };
                write!(f, "GitLab request {reason}: {endpoint}")
            }
            Self::Api {
                endpoint,
                status,
                rate_limited,
            } => {
                let reason = if *rate_limited {
                    "API rate limit exceeded"
                } else {
                    match status {
                        401 => "authentication failed; check GITLAB_TOKEN",
                        403 => "access denied; check token permissions",
                        404 => "merge request or project not found or inaccessible",
                        _ => "API request failed",
                    }
                };
                write!(f, "GitLab {reason} (HTTP {status}): {endpoint}")
            }
            Self::Decode { endpoint, .. } => write!(f, "invalid GitLab response: {endpoint}"),
            Self::InvalidPagination => write!(f, "invalid GitLab pagination link"),
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
            Self::Request { source, .. } => Some(source),
            Self::Api { .. } => None,
            Self::Decode { source, .. } => Some(source),
            Self::InvalidPagination => None,
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
