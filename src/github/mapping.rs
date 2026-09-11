use log::trace;

use crate::context::{ReviewCommentThread, ReviewContext, ReviewThreadComment};

use super::client::{IssueComment, PullRequest};

pub fn review_context(pull: PullRequest, mut comments: Vec<IssueComment>) -> ReviewContext {
    trace!(
        "mapping GitHub review context: conversation_comments={}",
        comments.len()
    );
    comments.sort_by(|left, right| (&left.created_at, left.id).cmp(&(&right.created_at, right.id)));
    ReviewContext {
        title: Some(pull.title),
        body: Some(pull.body.unwrap_or_default()),
        comments: comments
            .into_iter()
            .map(|comment| ReviewCommentThread {
                commit: None,
                location: None,
                comments: vec![ReviewThreadComment {
                    author: comment
                        .user
                        .map_or_else(|| "unknown".into(), |user| user.login),
                    body: comment.body,
                }],
            })
            .collect(),
    }
}
