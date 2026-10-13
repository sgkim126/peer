use std::num::NonZeroU64;

use crate::gh::GhClient;
use crate::github::Repository;
use crate::github::position::ChangedFile;

use super::{CommandError, CommitRef, PullRequest, api};

#[derive(Clone, Copy)]
pub struct PullRequestInput<'a> {
    pub repository: &'a Repository,
    pub number: NonZeroU64,
}

impl PullRequestInput<'_> {
    pub fn path(self) -> String {
        format!("repos/{}/pulls/{}", self.repository, self.number)
    }

    pub fn issue_path(self) -> String {
        format!("repos/{}/issues/{}", self.repository, self.number)
    }

    pub fn repo_arg(self) -> String {
        format!("github.com/{}", self.repository)
    }
}

pub async fn get_pull_request(
    client: &GhClient,
    input: PullRequestInput<'_>,
) -> Result<PullRequest, CommandError> {
    api::get(client, &input.path()).await
}

pub async fn list_pr_commits(
    client: &GhClient,
    input: PullRequestInput<'_>,
) -> Result<Vec<CommitRef>, CommandError> {
    api::list(client, &format!("{}/commits", input.path())).await
}

pub async fn list_pr_files(
    client: &GhClient,
    input: PullRequestInput<'_>,
) -> Result<Vec<ChangedFile>, CommandError> {
    api::list(client, &format!("{}/files", input.path())).await
}
