use serde::Deserialize;

use crate::git::CommitHash;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[allow(dead_code)]
pub struct ReviewContextInput {
    pub title: Option<String>,
    pub body: Option<String>,
    pub comments: Vec<ReviewComment>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[allow(dead_code)]
pub struct ReviewContext {
    pub title: Option<String>,
    pub body_summary: Option<String>,
    pub comments_summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ReviewComment {
    pub body: String,
    pub commit: Option<CommitHash>,
    pub location: Option<ReviewCommentLocation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ReviewCommentLocation {
    pub path: String,
    pub line: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comments(input: &str) -> Vec<ReviewComment> {
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
}
