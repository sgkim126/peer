use std::collections::HashSet;
use std::num::NonZeroU64;

use log::debug;
use serde::{Deserialize, Deserializer, Serialize, de::DeserializeOwned};

use crate::context::ReviewContext;
use crate::git::CommitHash;
use crate::glab::GlabClient;

use super::commands::{self, ListCommitDiscussionsInput, MergeRequest, MergeRequestInput};
use super::{GitLabError, Repository, mapping};

pub struct GitLabClient {
    pub glab: GlabClient,
}

impl Default for GitLabClient {
    fn default() -> Self {
        Self::new(GlabClient::default())
    }
}

impl GitLabClient {
    pub fn new(glab: GlabClient) -> Self {
        Self { glab }
    }

    pub async fn review_input(
        &self,
        repository: &Repository,
        number: NonZeroU64,
    ) -> Result<GitLabReviewInput, GitLabError> {
        debug!("loading GitLab review input: repository={repository} merge_request={number}");
        let merge_request = self.merge_request(repository, number).await?;
        let source = merge_request.source(number)?;
        let input = MergeRequestInput { repository, number };
        let commits: Vec<_> = commands::list_mr_commits(&self.glab, input)
            .await?
            .into_iter()
            .map(|commit| commit.id)
            .collect();
        let unique: HashSet<_> = commits.iter().map(CommitHash::as_ref).collect();
        if commits.is_empty()
            || unique.len() != commits.len()
            || !commits.contains(&source.diff_refs.head_sha)
        {
            return Err(GitLabError::IncompleteCommits);
        }
        let mut discussions = commands::list_mr_discussions(&self.glab, input).await?;
        let source_project_id = source
            .source_project_id
            .filter(|project_id| *project_id != source.project_id);
        let mut source_project_accessible = true;
        let mut first_source_request = true;
        for commit in &commits {
            discussions.extend(
                commands::list_commit_discussions(
                    &self.glab,
                    ListCommitDiscussionsInput {
                        project_id: source.project_id,
                        commit,
                        skip_inaccessible_project: false,
                    },
                )
                .await?
                .items,
            );
            if let Some(project_id) = source_project_id
                && source_project_accessible
            {
                // A token scoped to the target may not be able to read the fork.
                // Only the first request can skip an inaccessible fork; later failures
                // must abort the review so partially loaded comments are never used.
                let comments = commands::list_commit_discussions(
                    &self.glab,
                    ListCommitDiscussionsInput {
                        project_id,
                        commit,
                        skip_inaccessible_project: first_source_request,
                    },
                )
                .await?;
                first_source_request = false;
                source_project_accessible = comments.project_accessible;
                discussions.extend(comments.items);
            }
        }
        let current = self.merge_request(repository, number).await?;
        if current.source(number)? != source
            || current.source_branch != merge_request.source_branch
            || current.target_branch != merge_request.target_branch
        {
            return Err(GitLabError::MergeRequestChanged);
        }
        // GitLab's commit list order is not part of the API contract. The local
        // target resolver validates membership and orders this set using Git.
        let context = mapping::review_context(merge_request, discussions);
        debug!(
            "loaded GitLab review input: repository={repository} merge_request={number} commits={} threads={}",
            commits.len(),
            context.comments.len()
        );
        Ok(GitLabReviewInput {
            context,
            commits,
            source,
        })
    }

    pub async fn merge_request(
        &self,
        repository: &Repository,
        number: NonZeroU64,
    ) -> Result<MergeRequest, GitLabError> {
        Ok(
            commands::get_merge_request(&self.glab, MergeRequestInput { repository, number })
                .await?,
        )
    }

    pub async fn list<T: DeserializeOwned>(&self, path: &str) -> Result<Vec<T>, GitLabError> {
        Ok(commands::api::list(&self.glab, path).await?)
    }

    pub async fn post<T: DeserializeOwned>(
        &self,
        path: &str,
        body: &impl Serialize,
    ) -> Result<T, GitLabError> {
        Ok(commands::api::post(&self.glab, path, body).await?)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DiffRefs {
    pub base_sha: CommitHash,
    pub head_sha: CommitHash,
    pub start_sha: CommitHash,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "provider", rename = "gitlab")]
pub struct GitLabReviewSource {
    pub project_id: u64,
    pub iid: u64,
    pub source_project_id: Option<u64>,
    pub diff_refs: DiffRefs,
}

impl<'de> Deserialize<'de> for GitLabReviewSource {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(tag = "provider", rename_all = "lowercase", deny_unknown_fields)]
        enum Source {
            Gitlab {
                project_id: u64,
                iid: u64,
                source_project_id: Option<u64>,
                diff_refs: DiffRefs,
            },
        }
        let Source::Gitlab {
            project_id,
            iid,
            source_project_id,
            diff_refs,
        } = Source::deserialize(deserializer)?;
        Ok(Self {
            project_id,
            iid,
            source_project_id,
            diff_refs,
        })
    }
}

#[derive(Debug)]
pub struct GitLabReviewInput {
    pub context: ReviewContext,
    pub commits: Vec<CommitHash>,
    pub source: GitLabReviewSource,
}

impl MergeRequest {
    pub fn source(&self, number: NonZeroU64) -> Result<GitLabReviewSource, GitLabError> {
        if self.project_id == 0 {
            return Err(GitLabError::InvalidMergeRequest);
        }
        if self.project_id != self.target_project_id {
            return Err(GitLabError::InvalidMergeRequest);
        }
        if self.iid != number.get() {
            return Err(GitLabError::InvalidMergeRequest);
        }
        if self.source_project_id == Some(0) {
            return Err(GitLabError::InvalidMergeRequest);
        }
        let diff_refs = self
            .diff_refs
            .as_ref()
            .ok_or(GitLabError::MergeRequestNotReady)?;
        if self.sha.as_ref() != Some(&diff_refs.head_sha) {
            return Err(GitLabError::MergeRequestNotReady);
        }
        Ok(GitLabReviewSource {
            project_id: self.project_id,
            iid: self.iid,
            source_project_id: self.source_project_id,
            diff_refs: diff_refs.clone(),
        })
    }
}

#[cfg(test)]
mod tests;
