use std::num::NonZeroU64;

use crate::git::CommitHash;
use crate::glab::GlabClient;

use super::super::Repository;
use super::{ChangedFile, CommandError, CommitRef, MergeRequest, api};

#[derive(Clone, Copy)]
pub struct MergeRequestInput<'a> {
    pub repository: &'a Repository,
    pub number: NonZeroU64,
}

impl MergeRequestInput<'_> {
    pub fn path(self) -> String {
        format!(
            "{}/merge_requests/{}",
            self.repository.api_path(),
            self.number
        )
    }
}

#[derive(Clone, Copy)]
pub struct CommitInput<'a> {
    pub repository: &'a Repository,
    pub commit: &'a CommitHash,
}

impl CommitInput<'_> {
    pub fn path(self) -> String {
        format!(
            "{}/repository/commits/{}",
            self.repository.api_path(),
            self.commit
        )
    }
}

pub async fn get_merge_request(
    client: &GlabClient,
    input: MergeRequestInput<'_>,
) -> Result<MergeRequest, CommandError> {
    api::get(client, &input.path()).await
}

pub async fn list_mr_commits(
    client: &GlabClient,
    input: MergeRequestInput<'_>,
) -> Result<Vec<CommitRef>, CommandError> {
    api::list(client, &format!("{}/commits", input.path())).await
}

pub async fn list_commit_diff(
    client: &GlabClient,
    input: CommitInput<'_>,
) -> Result<Vec<ChangedFile>, CommandError> {
    api::list(client, &format!("{}/diff", input.path())).await
}
