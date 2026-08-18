use std::fmt;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::git::CommitHash;

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<ReviewCommentThread>,
}

impl ReviewContext {
    pub fn load(
        title: Option<String>,
        body_file: Option<&Path>,
        comments_file: Option<&Path>,
    ) -> Result<Self, ReviewContextError> {
        let body = body_file
            .map(|path| {
                std::fs::read_to_string(path).map_err(|source| ReviewContextError::ReadBody {
                    path: path.to_path_buf(),
                    source,
                })
            })
            .transpose()?;
        let comments = comments_file
            .map(|path| {
                let input = std::fs::read_to_string(path).map_err(|source| {
                    ReviewContextError::ReadComments {
                        path: path.to_path_buf(),
                        source,
                    }
                })?;
                serde_json::from_str(&input).map_err(|source| ReviewContextError::ParseComments {
                    path: path.to_path_buf(),
                    source,
                })
            })
            .transpose()?
            .unwrap_or_default();

        Ok(Self {
            title,
            body,
            comments,
        })
    }
}

#[derive(Debug)]
pub enum ReviewContextError {
    ReadBody {
        path: PathBuf,
        source: std::io::Error,
    },
    ReadComments {
        path: PathBuf,
        source: std::io::Error,
    },
    ParseComments {
        path: PathBuf,
        source: serde_json::Error,
    },
}

impl fmt::Display for ReviewContextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadBody { path, .. } => {
                write!(f, "failed to read review body file {}", path.display())
            }
            Self::ReadComments { path, .. } => {
                write!(f, "failed to read review comments file {}", path.display())
            }
            Self::ParseComments { path, .. } => {
                write!(f, "failed to parse review comments file {}", path.display())
            }
        }
    }
}

impl std::error::Error for ReviewContextError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ReadBody { source, .. } => Some(source),
            Self::ReadComments { source, .. } => Some(source),
            Self::ParseComments { source, .. } => Some(source),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ReviewCommentThread {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<CommitHash>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<ReviewCommentLocation>,
    pub comments: Vec<ReviewThreadComment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ReviewThreadComment {
    pub author: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ReviewCommentLocation {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<NonZeroU32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::assert_matches;

    #[test]
    fn loads_context_files_and_validates_comments() {
        let directory = tempfile::tempdir().unwrap();
        let body_path = directory.path().join("body.md");
        let comments_path = directory.path().join("comments.json");
        std::fs::write(&body_path, "Body text").unwrap();
        std::fs::write(
            &comments_path,
            r#"[{"comments":[{"author":"alice","body":"Comment text"}]}]"#,
        )
        .unwrap();

        let context = ReviewContext::load(
            Some("Title".to_string()),
            Some(&body_path),
            Some(&comments_path),
        )
        .unwrap();

        assert_eq!(context.title.as_deref(), Some("Title"));
        assert_eq!(context.body.as_deref(), Some("Body text"));
        assert_eq!(context.comments[0].comments[0].author, "alice");
    }

    #[test]
    fn rejects_invalid_comments() {
        let directory = tempfile::tempdir().unwrap();
        let comments_path = directory.path().join("comments.json");
        std::fs::write(
            &comments_path,
            r#"[{"comments":[{"body":"missing author"}]}]"#,
        )
        .unwrap();

        let error = ReviewContext::load(None, None, Some(&comments_path)).unwrap_err();
        assert_matches!(error, ReviewContextError::ParseComments { .. });
    }

    #[test]
    fn rejects_invalid_comment_commit() {
        let directory = tempfile::tempdir().unwrap();
        let comments_path = directory.path().join("comments.json");
        std::fs::write(
            &comments_path,
            r#"[{"commit":"","comments":[{"author":"alice","body":"comment"}]}]"#,
        )
        .unwrap();

        let error = ReviewContext::load(None, None, Some(&comments_path)).unwrap_err();
        assert_matches!(error, ReviewContextError::ParseComments { .. });
    }

    #[test]
    fn accepts_location_without_line() {
        let threads: Vec<ReviewCommentThread> =
            serde_json::from_str(r#"[{"location":{"path":"src/lib.rs"},"comments":[]}]"#).unwrap();

        assert_eq!(threads[0].location.as_ref().unwrap().line, None);
        let value = serde_json::to_value(&threads).unwrap();
        assert_eq!(value[0]["location"]["path"], "src/lib.rs");
        assert_eq!(value[0]["location"].get("line"), None);
    }

    #[test]
    fn rejects_zero_location_line() {
        let error = serde_json::from_str::<Vec<ReviewCommentThread>>(
            r#"[{"location":{"path":"src/lib.rs","line":0},"comments":[]}]"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("nonzero"));
    }

    #[test]
    fn reports_missing_input_files() {
        let body_error =
            ReviewContext::load(None, Some(Path::new("missing-review-body.md")), None).unwrap_err();
        assert_matches!(body_error, ReviewContextError::ReadBody { .. });

        let comments_error =
            ReviewContext::load(None, None, Some(Path::new("missing-review-comments.json")))
                .unwrap_err();
        assert_matches!(comments_error, ReviewContextError::ReadComments { .. });
    }
}
