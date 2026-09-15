use std::collections::BTreeMap;

use log::{debug, trace};

use crate::context::{
    ReviewCommentLocation, ReviewCommentThread, ReviewContext, ReviewThreadComment,
};

use super::client::{IssueComment, PullRequest, PullRequestReview, ReviewComment, User};
use super::publish::CONVERSATION_MARKER;

pub fn review_context(
    pull: PullRequest,
    mut comments: Vec<IssueComment>,
    mut reviews: Vec<PullRequestReview>,
    review_comments: Vec<ReviewComment>,
) -> ReviewContext {
    trace!(
        "mapping GitHub review context: conversation_comments={} reviews={} review_comments={}",
        comments.len(),
        reviews.len(),
        review_comments.len()
    );
    comments.retain(|comment| {
        if is_peer_conversation(&comment.body) {
            trace!(
                "skipping GitHub conversation comment: comment_id={} reason=peer_conversation",
                comment.id
            );
            false
        } else {
            true
        }
    });
    let conversation_threads = comments.len();
    let review_count = reviews.len();
    comments.sort_by(|left, right| (&left.created_at, left.id).cmp(&(&right.created_at, right.id)));
    let mut threads: Vec<_> = comments
        .into_iter()
        .map(|comment| ReviewCommentThread {
            commit: None,
            location: None,
            comments: vec![thread_comment(comment.user, comment.body)],
        })
        .collect();

    reviews
        .sort_by(|left, right| (&left.submitted_at, left.id).cmp(&(&right.submitted_at, right.id)));
    threads.extend(reviews.into_iter().filter_map(|review| {
        if review.state == "PENDING" || review.submitted_at.is_none() {
            trace!(
                "skipping GitHub review: review_id={} reason=unsubmitted",
                review.id
            );
            return None;
        }
        let Some(body) = review.body.filter(|body| !body.trim().is_empty()) else {
            trace!(
                "skipping GitHub review: review_id={} reason=empty_body",
                review.id
            );
            return None;
        };
        Some(ReviewCommentThread {
            commit: review.commit_id,
            location: None,
            comments: vec![thread_comment(review.user, body)],
        })
    }));
    let review_threads = threads.len() - conversation_threads;

    // GitHub's in_reply_to_id identifies the top-level comment of a thread.
    // Group only after all pages are available, including replies whose root is gone.
    let mut groups: BTreeMap<u64, Vec<ReviewComment>> = BTreeMap::new();
    for comment in review_comments {
        groups
            .entry(comment.in_reply_to_id.unwrap_or(comment.id))
            .or_default()
            .push(comment);
    }
    let mut inline_threads = Vec::with_capacity(groups.len());
    for (root_id, mut comments) in groups {
        comments
            .sort_by(|left, right| (&left.created_at, left.id).cmp(&(&right.created_at, right.id)));
        let root = comments
            .iter()
            .find(|comment| comment.id == root_id)
            .unwrap_or_else(|| {
                debug!(
                    "GitHub thread root missing: root_id={root_id} fallback_comment_id={}",
                    comments[0].id
                );
                &comments[0]
            });
        let sort_key = (root.created_at.clone(), root_id);
        let mut commit = root.commit_id.clone();
        let mut line = None;
        if root.side.as_deref() == Some("RIGHT") {
            if let Some(current_line) = root.line {
                line = Some(current_line);
            } else if let (Some(original_line), Some(original_commit)) =
                (root.original_line, &root.original_commit_id)
            {
                line = Some(original_line);
                commit = original_commit.clone();
                trace!(
                    "using original GitHub inline comment location: root_id={root_id} line={original_line}"
                );
            }
        }
        let location = (!root.path.is_empty()).then(|| ReviewCommentLocation {
            path: root.path.clone(),
            line,
        });
        let thread = ReviewCommentThread {
            commit: Some(commit),
            location,
            comments: comments
                .into_iter()
                .map(|comment| thread_comment(comment.user, comment.body))
                .collect(),
        };
        trace!(
            "mapped GitHub inline thread: root_id={root_id} comments={} line={:?}",
            thread.comments.len(),
            thread.location.as_ref().and_then(|location| location.line)
        );
        inline_threads.push((sort_key, thread));
    }
    inline_threads.sort_by(|left, right| left.0.cmp(&right.0));
    let inline_thread_count = inline_threads.len();
    threads.extend(inline_threads.into_iter().map(|(_, thread)| thread));

    debug!(
        "mapped GitHub review context: conversation_threads={conversation_threads} review_threads={review_threads} skipped_reviews={} inline_threads={inline_thread_count}",
        review_count - review_threads
    );
    ReviewContext {
        title: Some(pull.title),
        body: Some(pull.body.unwrap_or_default()),
        comments: threads,
    }
}

fn is_peer_conversation(body: &str) -> bool {
    body.lines().any(|line| line.trim() == CONVERSATION_MARKER)
}

fn thread_comment(user: Option<User>, body: String) -> ReviewThreadComment {
    ReviewThreadComment {
        author: user.map_or_else(
            || {
                trace!("GitHub comment author missing; using unknown");
                "unknown".into()
            },
            |user| user.login,
        ),
        body,
    }
}

#[cfg(test)]
mod tests;
