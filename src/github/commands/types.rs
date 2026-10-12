use serde::Deserialize;

use crate::git::CommitHash;

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
