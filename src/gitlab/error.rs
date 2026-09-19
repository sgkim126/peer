use std::{env::VarError, fmt};

#[derive(Debug)]
#[expect(dead_code)]
pub enum GitLabError {
    MissingRepository,
    InvalidRepository,
    MissingToken,
    InvalidToken,
    Client(reqwest::Error),
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
