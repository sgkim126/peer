use std::fmt;

#[derive(Debug)]
#[expect(dead_code)]
pub enum GitLabError {
    MissingRepository,
    InvalidRepository,
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
        }
    }
}

impl std::error::Error for GitLabError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::MissingRepository => None,
            Self::InvalidRepository => None,
        }
    }
}
