use std::num::NonZeroU64;

use crate::gh::GhClient;
use crate::github::Repository;
use crate::github::position::ChangedFile;

use super::{CommandError, CommitRef, PullRequest, api};

#[derive(Clone, Copy)]
#[cfg_attr(not(test), expect(dead_code))]
pub struct PullRequestInput<'a> {
    pub repository: &'a Repository,
    pub number: NonZeroU64,
}

impl PullRequestInput<'_> {
    #[cfg_attr(not(test), expect(dead_code))]
    pub fn path(self) -> String {
        format!("repos/{}/pulls/{}", self.repository, self.number)
    }

    pub fn repo_arg(self) -> String {
        format!("github.com/{}", self.repository)
    }
}

#[cfg_attr(not(test), expect(dead_code))]
pub async fn get_pull_request(
    client: &GhClient,
    input: PullRequestInput<'_>,
) -> Result<PullRequest, CommandError> {
    api::get(client, &input.path()).await
}

#[cfg_attr(not(test), expect(dead_code))]
pub async fn list_pr_commits(
    client: &GhClient,
    input: PullRequestInput<'_>,
) -> Result<Vec<CommitRef>, CommandError> {
    api::list(client, &format!("{}/commits", input.path())).await
}

#[cfg_attr(not(test), expect(dead_code))]
pub async fn list_pr_files(
    client: &GhClient,
    input: PullRequestInput<'_>,
) -> Result<Vec<ChangedFile>, CommandError> {
    api::list(client, &format!("{}/files", input.path())).await
}
