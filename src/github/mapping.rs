use crate::context::ReviewContext;

use super::client::PullRequest;

pub fn review_context(pull: PullRequest) -> ReviewContext {
    ReviewContext {
        title: Some(pull.title),
        body: Some(pull.body.unwrap_or_default()),
        comments: Vec::new(),
    }
}
