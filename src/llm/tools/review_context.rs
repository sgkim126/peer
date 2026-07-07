#[cfg(test)]
use crate::llm::context::ReviewComment;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum ReviewContextSummaryKind {
    Body,
    Comments,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
struct ReviewContextSummaryInput {
    kind: ReviewContextSummaryKind,
    content: String,
}

impl ReviewContextSummaryInput {
    #[cfg(test)]
    fn comments(comments: &[ReviewComment]) -> Self {
        Self {
            kind: ReviewContextSummaryKind::Comments,
            content: format_comments(comments),
        }
    }
}

#[cfg(test)]
fn format_comments(comments: &[ReviewComment]) -> String {
    comments
        .iter()
        .enumerate()
        .map(|(index, comment)| format_comment(index + 1, comment))
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
fn format_comment(index: usize, comment: &ReviewComment) -> String {
    let mut output = format!("Comment {index}:");

    if let Some(commit) = &comment.commit {
        output.push_str("\nCommit: ");
        output.push_str(commit.as_ref());
    }
    if let Some(location) = &comment.location {
        output.push_str("\nLocation: ");
        output.push_str(&location.path);
        output.push(':');
        output.push_str(&location.line.to_string());
    }

    output.push_str("\nBody:\n");
    output.push_str(&comment.body);
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::CommitHash;
    use crate::llm::context::ReviewCommentLocation;

    #[test]
    fn formats_comments_with_optional_metadata() {
        let input = ReviewContextSummaryInput::comments(&[
            ReviewComment {
                body: "Please cover this branch.".to_string(),
                commit: Some(CommitHash::new("abc1234").unwrap()),
                location: Some(ReviewCommentLocation {
                    path: "src/lib.rs".to_string(),
                    line: 42,
                }),
            },
            ReviewComment {
                body: "This looks resolved.".to_string(),
                commit: None,
                location: None,
            },
        ]);

        assert_eq!(input.kind, ReviewContextSummaryKind::Comments);
        assert_eq!(
            input.content,
            "Comment 1:\nCommit: abc1234\nLocation: src/lib.rs:42\nBody:\nPlease cover this branch.\n\nComment 2:\nBody:\nThis looks resolved."
        );
    }
}
