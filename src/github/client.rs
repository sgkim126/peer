use std::collections::HashSet;
use std::num::NonZeroU64;

use log::debug;
use serde::{Deserialize, de::DeserializeOwned};

use crate::context::ReviewContext;
use crate::gh::GhClient;
use crate::git::CommitHash;

use super::commands::{
    self, Collection, CommitComment, IssueComment, PullRequest, PullRequestInput,
    PullRequestReview, ReviewComment,
};
use super::position::{ChangedFile, commit_comment_position};
use super::{GitHubError, Repository, mapping};

pub struct GitHubClient {
    pub gh: GhClient,
}

impl Default for GitHubClient {
    fn default() -> Self {
        Self::new(GhClient::default())
    }
}

impl GitHubClient {
    pub fn new(gh: GhClient) -> Self {
        Self { gh }
    }

    pub async fn review_input(
        &self,
        repository: &Repository,
        number: NonZeroU64,
    ) -> Result<GitHubReviewInput, GitHubError> {
        debug!("loading GitHub review input: repository={repository} pull_request={number}");
        let input = PullRequestInput { repository, number };
        let pull = commands::get_pull_request(&self.gh, input).await?;
        let source_repository = pull
            .head
            .repo
            .as_ref()
            .map(|repo| Repository::parse(&repo.full_name))
            .transpose()?;
        let prefix = format!("repos/{repository}");
        let comments =
            commands::list::<IssueComment>(&self.gh, &format!("{prefix}/issues/{number}/comments"))
                .await?;
        let reviews = commands::list::<PullRequestReview>(
            &self.gh,
            &format!("{prefix}/pulls/{number}/reviews"),
        )
        .await?;
        let review_comments =
            commands::list::<ReviewComment>(&self.gh, &format!("{prefix}/pulls/{number}/comments"))
                .await?;
        let commits = self.pull_request_commits(repository, number, &pull).await?;
        let source = source_repository.as_ref().filter(|source| {
            !source
                .to_string()
                .eq_ignore_ascii_case(&repository.to_string())
        });
        let mut source_repository_accessible = true;
        let mut first_source_request = true;
        let mut commit_comments = Vec::new();
        for commit in &commits {
            commit_comments.extend(self.commit_comments(repository, commit, false).await?.items);
            if let Some(source) = source
                && source_repository_accessible
            {
                // A token scoped to the target may not be able to read the fork.
                // Only the first request can skip an inaccessible fork; later failures
                // must abort the review so partially loaded comments are never used.
                let comments = self
                    .commit_comments(source, commit, first_source_request)
                    .await?;
                first_source_request = false;
                source_repository_accessible = comments.repository_accessible;
                commit_comments.extend(comments.items);
            }
        }
        // A base change can alter commit membership without changing the head or count.
        let current_pull = commands::get_pull_request(&self.gh, input).await?;
        if current_pull.base.sha != pull.base.sha
            || current_pull.head.sha != pull.head.sha
            || current_pull.head.repo != pull.head.repo
            || current_pull.commits != pull.commits
        {
            return Err(GitHubError::IncompleteCommits);
        }
        let context =
            mapping::review_context(pull, comments, reviews, review_comments, commit_comments);
        debug!(
            "loaded GitHub review input: repository={repository} pull_request={number} commits={} threads={}",
            commits.len(),
            context.comments.len()
        );
        Ok(GitHubReviewInput { context, commits })
    }

    async fn commit_comments(
        &self,
        repository: &Repository,
        commit: &CommitHash,
        skip_inaccessible_repository: bool,
    ) -> Result<Collection<CommitComment>, GitHubError> {
        let mut comments = commands::pages(
            &self.gh,
            &format!("repos/{repository}/commits/{commit}/comments"),
            skip_inaccessible_repository,
            |page: Vec<CommitComment>| page,
        )
        .await?;
        if !comments.items.iter().any(|comment| {
            comment.commit_id == *commit
                && comment.path.as_ref().is_some_and(|path| !path.is_empty())
                && comment.position.is_some_and(|position| position > 0)
        }) {
            return Ok(comments);
        }
        // Coordinates are optional context. A missing diff must not discard comments,
        // and no coordinates are resolved until every file page has loaded.
        match self.commit_files(repository, commit).await {
            Ok(files) => {
                for comment in &mut comments.items {
                    if comment.commit_id == *commit
                        && let Some(path) = &comment.path
                        && let Some(position) = comment.position
                    {
                        comment.resolved_position = commit_comment_position(&files, path, position);
                    }
                }
            }
            Err(error) => debug!(
                "GitHub commit comment coordinates unavailable: repository={repository} commit={commit} error={error}"
            ),
        }
        Ok(comments)
    }

    async fn commit_files(
        &self,
        repository: &Repository,
        commit: &CommitHash,
    ) -> Result<Vec<ChangedFile>, GitHubError> {
        Ok(commands::pages(
            &self.gh,
            &format!("repos/{repository}/commits/{commit}"),
            false,
            |page: CommitFiles| page.files,
        )
        .await?
        .items)
    }

    pub async fn pull_request_commits(
        &self,
        repository: &Repository,
        number: NonZeroU64,
        pull: &PullRequest,
    ) -> Result<Vec<CommitHash>, GitHubError> {
        let commits: Vec<_> =
            commands::list_pr_commits(&self.gh, PullRequestInput { repository, number })
                .await?
                .into_iter()
                .map(|commit| commit.sha)
                .collect();
        // Reject truncated lists and commits inconsistent with the initial PR.
        if commits.len() != pull.commits || commits.last() != Some(&pull.head.sha) {
            return Err(GitHubError::IncompleteCommits);
        }
        // Reject repeated pages.
        if commits
            .iter()
            .map(AsRef::as_ref)
            .collect::<HashSet<&str>>()
            .len()
            != commits.len()
        {
            return Err(GitHubError::IncompleteCommits);
        }
        Ok(commits)
    }

    pub async fn post<T: DeserializeOwned>(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<T, GitHubError> {
        Ok(commands::post(&self.gh, path, body).await?)
    }

    pub async fn pull_request(
        &self,
        repository: &Repository,
        number: NonZeroU64,
    ) -> Result<PullRequest, GitHubError> {
        Ok(commands::get_pull_request(&self.gh, PullRequestInput { repository, number }).await?)
    }
}

#[derive(Debug)]
pub struct GitHubReviewInput {
    pub context: ReviewContext,
    pub commits: Vec<CommitHash>,
}

#[derive(Deserialize)]
struct CommitFiles {
    files: Vec<ChangedFile>,
}

#[cfg(test)]
mod tests;
