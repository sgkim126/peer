use std::num::NonZeroU32;

use serde::Deserialize;

use crate::git::CommitHash;
use crate::github::position::CommitCommentPosition;

#[derive(Debug, Deserialize)]
pub struct PullRequest {
    pub title: String,
    pub body: Option<String>,
    pub base: CommitRef,
    pub head: CommitRef,
    pub commits: usize,
}

#[derive(Debug, Deserialize)]
pub struct CommitRef {
    pub sha: CommitHash,
    pub repo: Option<RepositoryRef>,
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
pub struct RepositoryRef {
    pub full_name: String,
}

#[derive(Debug, Deserialize)]
pub struct User {
    pub login: String,
}

#[derive(Debug, Deserialize)]
pub struct IssueComment {
    pub id: u64,
    pub created_at: String,
    pub user: Option<User>,
    pub body: String,
}

#[derive(Debug, Deserialize)]
pub struct PullRequestReview {
    pub id: u64,
    pub submitted_at: Option<String>,
    pub state: String,
    pub user: Option<User>,
    pub body: Option<String>,
    pub commit_id: Option<CommitHash>,
}

#[derive(Debug, Deserialize)]
pub struct ReviewComment {
    pub id: u64,
    pub in_reply_to_id: Option<u64>,
    pub created_at: String,
    pub user: Option<User>,
    pub body: String,
    pub path: String,
    pub commit_id: CommitHash,
    pub original_commit_id: Option<CommitHash>,
    pub line: Option<NonZeroU32>,
    pub original_line: Option<NonZeroU32>,
    pub side: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CommitComment {
    pub id: u64,
    pub created_at: String,
    pub user: Option<User>,
    pub body: String,
    pub path: Option<String>,
    pub commit_id: CommitHash,
    pub position: Option<u32>,
    // The API's line can refer to a deleted line, so only trust patch-derived positions.
    #[serde(skip)]
    pub resolved_position: Option<CommitCommentPosition>,
}
