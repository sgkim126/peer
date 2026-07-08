use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::git::CommitHash;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReviewContextInput {
    pub title: Option<String>,
    pub body: Option<String>,
    pub comments: Vec<ReviewComment>,
}

impl ReviewContextInput {
    pub fn load(
        title: Option<String>,
        body_file: Option<&Path>,
        comments_file: Option<&Path>,
    ) -> Result<Self, ReviewContextInputError> {
        let body = body_file
            .map(|path| {
                std::fs::read_to_string(path).map_err(|source| ReviewContextInputError::ReadBody {
                    path: path.to_path_buf(),
                    source,
                })
            })
            .transpose()?;
        let comments = comments_file
            .map(|path| {
                let input = std::fs::read_to_string(path).map_err(|source| {
                    ReviewContextInputError::ReadComments {
                        path: path.to_path_buf(),
                        source,
                    }
                })?;
                serde_json::from_str(&input).map_err(|source| {
                    ReviewContextInputError::ParseComments {
                        path: path.to_path_buf(),
                        source,
                    }
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
pub enum ReviewContextInputError {
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

impl fmt::Display for ReviewContextInputError {
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

impl std::error::Error for ReviewContextInputError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ReadBody { source, .. } | Self::ReadComments { source, .. } => Some(source),
            Self::ParseComments { source, .. } => Some(source),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ReviewContext {
    pub title: Option<String>,
    pub body_summary: Option<String>,
    pub comments_summary: Option<String>,
}

impl ReviewContext {
    pub fn append_to_prompt(&self, prompt: &mut String) {
        if self.is_empty() {
            return;
        }

        prompt.push_str("\n\nReview context:");
        if let Some(title) = &self.title {
            prompt.push_str("\nTitle:\n");
            prompt.push_str(title);
        }
        if let Some(body_summary) = &self.body_summary {
            prompt.push_str("\nBody summary:\n");
            prompt.push_str(body_summary);
        }
        if let Some(comments_summary) = &self.comments_summary {
            prompt.push_str("\nComments summary:\n");
            prompt.push_str(comments_summary);
        }
    }

    fn is_empty(&self) -> bool {
        self.title.is_none() && self.body_summary.is_none() && self.comments_summary.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ReviewComment {
    pub body: String,
    pub commit: Option<CommitHash>,
    pub location: Option<ReviewCommentLocation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[allow(dead_code)]
pub struct ReviewCommentThread {
    pub commit: Option<CommitHash>,
    pub location: Option<ReviewCommentLocation>,
    pub comments: Vec<ReviewThreadComment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ReviewThreadComment {
    pub author: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ReviewCommentLocation {
    pub path: String,
    pub line: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn comments(input: &str) -> Vec<ReviewComment> {
        serde_json::from_str(input).unwrap()
    }

    fn comment_threads(input: &str) -> Vec<ReviewCommentThread> {
        serde_json::from_str(input).unwrap()
    }

    #[test]
    fn parses_comments_with_commit_and_location() {
        let comments = comments(
            r#"[
                {
                    "body": "Please handle this error case.",
                    "commit": "abc1234",
                    "location": {
                        "path": "src/lib.rs",
                        "line": 42
                    }
                }
            ]"#,
        );

        assert_eq!(
            comments,
            vec![ReviewComment {
                body: "Please handle this error case.".to_string(),
                commit: Some(CommitHash::new("abc1234").unwrap()),
                location: Some(ReviewCommentLocation {
                    path: "src/lib.rs".to_string(),
                    line: 42,
                }),
            }]
        );
    }

    #[test]
    fn parses_comments_without_optional_metadata() {
        let comments = comments(
            r#"[
                {
                    "body": "This part is hard to follow."
                }
            ]"#,
        );

        assert_eq!(
            comments,
            vec![ReviewComment {
                body: "This part is hard to follow.".to_string(),
                commit: None,
                location: None,
            }]
        );
    }

    #[test]
    fn rejects_comment_without_body() {
        let error = serde_json::from_str::<Vec<ReviewComment>>(r#"[{}]"#).unwrap_err();

        assert!(error.to_string().contains("missing field `body`"));
    }

    #[test]
    fn rejects_invalid_comment_commit() {
        let error = serde_json::from_str::<Vec<ReviewComment>>(
            r#"[
                {
                    "body": "comment",
                    "commit": ""
                }
            ]"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("invalid commit hash"));
    }

    #[test]
    fn rejects_partial_location_without_line() {
        let error = serde_json::from_str::<Vec<ReviewComment>>(
            r#"[
                {
                    "body": "comment",
                    "location": {
                        "path": "src/lib.rs"
                    }
                }
            ]"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("missing field `line`"));
    }

    #[test]
    fn rejects_partial_location_without_path() {
        let error = serde_json::from_str::<Vec<ReviewComment>>(
            r#"[
                {
                    "body": "comment",
                    "location": {
                        "line": 42
                    }
                }
            ]"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("missing field `path`"));
    }

    #[test]
    fn parses_comment_thread_with_commit_location_and_comments() {
        let threads = comment_threads(
            r#"[
                {
                    "commit": "abc1234",
                    "location": {
                        "path": "src/lib.rs",
                        "line": 42
                    },
                    "comments": [
                        {
                            "author": "alice",
                            "body": "Please handle this error case."
                        },
                        {
                            "author": "bob",
                            "body": "Fixed in the latest push."
                        }
                    ]
                }
            ]"#,
        );

        assert_eq!(
            threads,
            vec![ReviewCommentThread {
                commit: Some(CommitHash::new("abc1234").unwrap()),
                location: Some(ReviewCommentLocation {
                    path: "src/lib.rs".to_string(),
                    line: 42,
                }),
                comments: vec![
                    ReviewThreadComment {
                        author: "alice".to_string(),
                        body: "Please handle this error case.".to_string(),
                    },
                    ReviewThreadComment {
                        author: "bob".to_string(),
                        body: "Fixed in the latest push.".to_string(),
                    },
                ],
            }]
        );
    }

    #[test]
    fn parses_comment_thread_without_optional_metadata() {
        let threads = comment_threads(
            r#"[
                {
                    "comments": [
                        {
                            "author": "alice",
                            "body": "This part is hard to follow."
                        }
                    ]
                }
            ]"#,
        );

        assert_eq!(
            threads,
            vec![ReviewCommentThread {
                commit: None,
                location: None,
                comments: vec![ReviewThreadComment {
                    author: "alice".to_string(),
                    body: "This part is hard to follow.".to_string(),
                }],
            }]
        );
    }

    #[test]
    fn rejects_thread_comment_without_author() {
        let error = serde_json::from_str::<Vec<ReviewCommentThread>>(
            r#"[
                {
                    "comments": [
                        {
                            "body": "comment"
                        }
                    ]
                }
            ]"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("missing field `author`"));
    }

    #[test]
    fn rejects_thread_comment_without_body() {
        let error = serde_json::from_str::<Vec<ReviewCommentThread>>(
            r#"[
                {
                    "comments": [
                        {
                            "author": "alice"
                        }
                    ]
                }
            ]"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("missing field `body`"));
    }

    #[test]
    fn rejects_comment_thread_with_invalid_commit() {
        let error = serde_json::from_str::<Vec<ReviewCommentThread>>(
            r#"[
                {
                    "commit": "",
                    "comments": [
                        {
                            "author": "alice",
                            "body": "comment"
                        }
                    ]
                }
            ]"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("invalid commit hash"));
    }

    #[test]
    fn loads_review_context_input_from_files() {
        let directory = tempfile::tempdir().unwrap();
        let body_path = directory.path().join("body.md");
        let comments_path = directory.path().join("comments.json");
        fs::write(&body_path, "This PR adds review context.").unwrap();
        fs::write(
            &comments_path,
            r#"[
                {
                    "body": "Please handle this error case.",
                    "commit": "abc1234"
                }
            ]"#,
        )
        .unwrap();

        let input = ReviewContextInput::load(
            Some("Add review context".to_string()),
            Some(&body_path),
            Some(&comments_path),
        )
        .unwrap();

        assert_eq!(input.title.as_deref(), Some("Add review context"));
        assert_eq!(input.body.as_deref(), Some("This PR adds review context."));
        assert_eq!(input.comments.len(), 1);
        assert_eq!(input.comments[0].body, "Please handle this error case.");
    }

    #[test]
    fn loads_review_context_input_without_optional_files() {
        let input = ReviewContextInput::load(None, None, None).unwrap();

        assert_eq!(input, ReviewContextInput::default());
    }

    #[test]
    fn fails_when_body_file_cannot_be_read() {
        let error =
            ReviewContextInput::load(None, Some(Path::new("missing-body.md")), None).unwrap_err();

        assert!(matches!(error, ReviewContextInputError::ReadBody { .. }));
    }

    #[test]
    fn fails_when_comments_file_cannot_be_parsed() {
        let directory = tempfile::tempdir().unwrap();
        let comments_path = directory.path().join("comments.json");
        fs::write(&comments_path, "not json").unwrap();

        let error = ReviewContextInput::load(None, None, Some(&comments_path)).unwrap_err();

        assert!(matches!(
            error,
            ReviewContextInputError::ParseComments { .. }
        ));
    }

    #[test]
    fn empty_review_context_does_not_change_prompt() {
        let original_prompt = "Review the following required commit data.";
        let mut prompt = original_prompt.to_string();

        ReviewContext::default().append_to_prompt(&mut prompt);

        assert_eq!(prompt, original_prompt);
    }

    #[test]
    fn appends_review_context_prompt_section() {
        let mut prompt = "Review the following required commit data.".to_string();

        ReviewContext {
            title: Some("Add review context".to_string()),
            body_summary: Some("Adds PR context support.".to_string()),
            comments_summary: Some("Reviewer asked for better error handling.".to_string()),
        }
        .append_to_prompt(&mut prompt);

        assert!(prompt.contains("Review context:"));
        assert!(prompt.contains("Title:\nAdd review context"));
        assert!(prompt.contains("Body summary:\nAdds PR context support."));
        assert!(prompt.contains("Comments summary:\nReviewer asked for better error handling."));
    }
}
