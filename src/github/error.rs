use std::{env::VarError, fmt};

#[derive(Debug)]
pub enum GitHubError {
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
    IncompleteCommits,
}

impl fmt::Display for GitHubError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingRepository => {
                write!(f, "--github requires [github].repo in .peer/config.toml")
            }
            Self::InvalidRepository => write!(f, "GitHub repository must use the form owner/name"),
            Self::MissingToken => write!(
                f,
                "--github requires a non-empty GITHUB_TOKEN environment variable"
            ),
            Self::InvalidToken => write!(f, "GITHUB_TOKEN is not a valid HTTP bearer token"),
            Self::Client(_) => write!(f, "failed to configure the GitHub HTTP client"),
            Self::Request { endpoint, source } => {
                let reason = if source.is_timeout() {
                    "timed out"
                } else {
                    "failed"
                };
                write!(f, "GitHub request {reason}: {endpoint}")
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
                        401 => "authentication failed; check GITHUB_TOKEN",
                        403 => "access denied; check token permissions",
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
        }
    }
}

impl std::error::Error for GitHubError {
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
            Self::IncompleteCommits => None,
        }
    }
}

impl From<VarError> for GitHubError {
    fn from(error: VarError) -> Self {
        match error {
            VarError::NotPresent => Self::MissingToken,
            VarError::NotUnicode(_) => Self::InvalidToken,
        }
    }
}

impl From<reqwest::Error> for GitHubError {
    fn from(error: reqwest::Error) -> Self {
        Self::Client(error)
    }
}
