use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::git::{CommitHash, GitError, run_git};

mod input;
mod pipeline;

pub use self::input::{ReviewCommitInput, ReviewInput};
pub use self::pipeline::{
    PipelineExecutionError, PipelineReviewResult, PipelineStageResult, run_pipeline,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewTarget {
    Commit(CommitHash),
    Range {
        from: CommitHash,
        to: CommitHash,
        commits: Vec<CommitHash>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSummary {
    pub peer_version: String,
}

pub async fn resolve_target(
    target: &str,
    max_commits: u32,
    project_root: &Path,
) -> Result<ReviewTarget, ReviewTargetError> {
    if !target.contains("..") {
        return Ok(ReviewTarget::Commit(
            CommitHash::resolve(target, project_root).await?,
        ));
    }

    if target.contains("...") {
        return Err(ReviewTargetError::InvalidRange(target.to_string()));
    }
    let Some((from, to)) = target.split_once("..") else {
        return Err(ReviewTargetError::InvalidRange(target.to_string()));
    };
    if from.is_empty() || to.is_empty() || to.contains("..") {
        return Err(ReviewTargetError::InvalidRange(target.to_string()));
    }

    // Resolve both ends explicitly so invalid revisions produce the same useful
    // error as a single-commit target instead of leaking `git rev-list` stderr.
    let from = CommitHash::resolve(from, project_root).await?;
    let to = CommitHash::resolve(to, project_root).await?;
    let revision = format!("{from}..{to}");
    let commit_limit = u64::from(max_commits) + 1;
    let output = run_git(
        &[
            "rev-list",
            "--reverse",
            "--max-count",
            &format!("{commit_limit}"),
            &revision,
        ],
        project_root,
    )
    .await?;
    let commits = output
        .lines()
        .filter(|line| !line.is_empty())
        .map(CommitHash::new)
        .collect::<Result<Vec<_>, _>>()?;
    if commits.len() > max_commits as usize {
        return Err(ReviewTargetError::TooManyCommits {
            actual: commits.len(),
            maximum: max_commits,
        });
    }
    if commits.is_empty() {
        return Err(ReviewTargetError::EmptyRange(target.to_string()));
    }

    Ok(ReviewTarget::Range { from, to, commits })
}

pub async fn resolve_pull_request_target(
    mut commits: Vec<CommitHash>,
    max_commits: u32,
    project_root: &Path,
) -> Result<ReviewTarget, ReviewTargetError> {
    if commits.len() > max_commits as usize {
        return Err(ReviewTargetError::TooManyCommits {
            actual: commits.len(),
            maximum: max_commits,
        });
    }
    if commits.is_empty() {
        return Err(ReviewTargetError::EmptyRange("pull request".into()));
    }
    for commit in &mut commits {
        *commit = CommitHash::resolve(commit.as_ref(), project_root).await?;
    }

    let first = &commits[0];
    if commits.len() == 1 {
        return Ok(ReviewTarget::Commit(first.clone()));
    }

    // The first PR commit's parent is the base of its cumulative diff, even
    // when the base branch has advanced or already merged the PR.
    let from = CommitHash::resolve(&format!("{first}^"), project_root).await?;
    let to = commits.last().expect("nonempty PR commit list").clone();
    Ok(ReviewTarget::Range { from, to, commits })
}

pub async fn validate_target(
    target: &ReviewTarget,
    max_commits: u32,
    project_root: &Path,
) -> Result<(), ReviewTargetError> {
    let commits = match target {
        ReviewTarget::Commit(commit) => std::slice::from_ref(commit),
        ReviewTarget::Range { commits, .. } => commits,
    };

    if commits.len() > max_commits as usize {
        return Err(ReviewTargetError::TooManyCommits {
            actual: commits.len(),
            maximum: max_commits,
        });
    }

    for commit in commits {
        let output = run_git(
            &["rev-list", "--parents", "-n", "1", commit.as_ref()],
            project_root,
        )
        .await?;
        if output.split_whitespace().count() > 2 {
            return Err(ReviewTargetError::MergeCommit(commit.clone()));
        }
    }

    Ok(())
}

/// Resolve GitLab's unordered API list against the actual MR graph locally.
#[cfg_attr(not(test), expect(dead_code))]
pub async fn resolve_merge_request_target(
    commits: Vec<CommitHash>,
    refs: &crate::gitlab::DiffRefs,
    max_commits: u32,
    project_root: &Path,
) -> Result<ReviewTarget, ReviewTargetError> {
    if commits.len() > max_commits as usize {
        return Err(ReviewTargetError::TooManyCommits {
            actual: commits.len(),
            maximum: max_commits,
        });
    }
    let base = CommitHash::resolve(refs.base_sha.as_ref(), project_root)
        .await
        .map_err(ReviewTargetError::MissingMergeRequestCommit)?;
    let head = CommitHash::resolve(refs.head_sha.as_ref(), project_root)
        .await
        .map_err(ReviewTargetError::MissingMergeRequestCommit)?;
    let merge_base = run_git(&["merge-base", base.as_ref(), head.as_ref()], project_root).await?;
    if merge_base.trim() != base.as_ref() {
        return Err(ReviewTargetError::IncompleteMergeRequest);
    }
    let mut remote = std::collections::HashSet::new();
    for commit in &commits {
        let commit = CommitHash::resolve(commit.as_ref(), project_root)
            .await
            .map_err(ReviewTargetError::MissingMergeRequestCommit)?;
        if !remote.insert(commit.to_string()) {
            return Err(ReviewTargetError::IncompleteMergeRequest);
        }
    }
    let target = resolve_target(&format!("{base}..{head}"), max_commits, project_root).await?;
    let ReviewTarget::Range {
        commits: ordered, ..
    } = &target
    else {
        unreachable!("two-dot target")
    };
    if ordered.len() != remote.len()
        || ordered
            .iter()
            .any(|commit| !remote.contains(commit.as_ref()))
    {
        return Err(ReviewTargetError::IncompleteMergeRequest);
    }
    if ordered.len() == 1 {
        Ok(ReviewTarget::Commit(ordered[0].clone()))
    } else {
        Ok(target)
    }
}

#[derive(Debug)]
pub enum ReviewTargetError {
    Git(GitError),
    InvalidRange(String),
    EmptyRange(String),
    TooManyCommits { actual: usize, maximum: u32 },
    MergeCommit(CommitHash),
    IncompleteMergeRequest,
    MissingMergeRequestCommit(GitError),
}

impl fmt::Display for ReviewTargetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Git(error) => error.fmt(f),
            Self::InvalidRange(range) => write!(f, "{range} is not a two-dot range"),
            Self::EmptyRange(range) => write!(f, "{range} contains no commits"),
            Self::TooManyCommits { actual, maximum } => {
                write!(
                    f,
                    "review target contains at least {actual} commits (max: {maximum})"
                )
            }
            Self::MergeCommit(commit) => write!(f, "review target contains merge commit {commit}"),
            Self::IncompleteMergeRequest => write!(
                f,
                "GitLab merge request commits do not match its diff; fetch the MR and retry"
            ),
            Self::MissingMergeRequestCommit(error) => write!(
                f,
                "GitLab merge request commits are unavailable locally; fetch the MR before reviewing: {error}"
            ),
        }
    }
}

impl std::error::Error for ReviewTargetError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Git(error) | Self::MissingMergeRequestCommit(error) => Some(error),
            Self::InvalidRange(_) => None,
            Self::EmptyRange(_) => None,
            Self::TooManyCommits { .. } => None,
            Self::MergeCommit(_) => None,
            Self::IncompleteMergeRequest => None,
        }
    }
}

impl From<GitError> for ReviewTargetError {
    fn from(error: GitError) -> Self {
        Self::Git(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::assert_matches;
    use std::path::PathBuf;

    use tempfile::TempDir;

    struct Repo {
        _tmp: TempDir,
        path: PathBuf,
    }

    impl Repo {
        async fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().to_path_buf();
            run_git(&["init"], &path).await.unwrap();
            run_git(&["config", "user.email", "test@example.com"], &path)
                .await
                .unwrap();
            run_git(&["config", "user.name", "Test"], &path)
                .await
                .unwrap();
            Self { _tmp: tmp, path }
        }

        async fn commit(&self, file: &str, message: &str) -> CommitHash {
            std::fs::write(self.path.join(file), message).unwrap();
            run_git(&["add", file], &self.path).await.unwrap();
            run_git(&["commit", "--no-gpg-sign", "-m", message], &self.path)
                .await
                .unwrap();
            CommitHash::resolve("HEAD", &self.path).await.unwrap()
        }
    }

    #[tokio::test]
    async fn gitlab_orders_the_graph_and_rejects_incomplete_or_duplicate_commits() {
        let repo = Repo::new().await;
        let base = repo.commit("base.txt", "base").await;
        let first = repo.commit("one.txt", "first").await;
        let head = repo.commit("two.txt", "second").await;
        run_git(
            &["checkout", "-b", "advanced-target", base.as_ref()],
            &repo.path,
        )
        .await
        .unwrap();
        let target_head = repo.commit("target.txt", "unrelated target change").await;
        let refs = crate::gitlab::DiffRefs {
            base_sha: base.clone(),
            start_sha: target_head,
            head_sha: head.clone(),
        };
        let target =
            resolve_merge_request_target(vec![head.clone(), first.clone()], &refs, 10, &repo.path)
                .await
                .unwrap();
        assert_eq!(
            target,
            ReviewTarget::Range {
                from: base,
                to: head.clone(),
                commits: vec![first.clone(), head.clone()]
            }
        );
        for commits in [vec![head.clone()], vec![first.clone(), head.clone(), first]] {
            assert_matches!(
                resolve_merge_request_target(commits, &refs, 10, &repo.path).await,
                Err(ReviewTargetError::IncompleteMergeRequest)
            );
        }
        assert_matches!(
            resolve_merge_request_target(vec![head.clone(), head], &refs, 1, &repo.path).await,
            Err(ReviewTargetError::TooManyCommits { .. })
        );
    }

    #[tokio::test]
    async fn gitlab_requires_local_objects_and_preserves_single_commit_reviews() {
        let repo = Repo::new().await;
        let base = repo.commit("base.txt", "base").await;
        let head = repo.commit("mr.txt", "change").await;
        let mut refs = crate::gitlab::DiffRefs {
            base_sha: base.clone(),
            start_sha: base,
            head_sha: head.clone(),
        };
        assert_eq!(
            resolve_merge_request_target(vec![head.clone()], &refs, 10, &repo.path)
                .await
                .unwrap(),
            ReviewTarget::Commit(head.clone())
        );
        refs.head_sha = CommitHash::new(&"a".repeat(40)).unwrap();
        let error = resolve_merge_request_target(vec![head], &refs, 10, &repo.path)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("fetch the MR"));
    }

    #[tokio::test]
    async fn pull_request_uses_its_commits_and_diff_even_after_the_base_branch_merges_it() {
        let repo = Repo::new().await;
        let base = repo.commit("base.txt", "base").await;
        run_git(&["checkout", "-b", "pull-request"], &repo.path)
            .await
            .unwrap();
        let first = repo.commit("first.txt", "first PR commit").await;
        let second = repo.commit("second.txt", "second PR commit").await;
        run_git(&["checkout", "-b", "base", base.as_ref()], &repo.path)
            .await
            .unwrap();
        repo.commit("unrelated.txt", "unrelated base change").await;

        for merged in [false, true] {
            if merged {
                run_git(
                    &[
                        "merge",
                        "--no-ff",
                        "--no-edit",
                        "--no-gpg-sign",
                        "pull-request",
                    ],
                    &repo.path,
                )
                .await
                .unwrap();
            }
            let target =
                resolve_pull_request_target(vec![first.clone(), second.clone()], 10, &repo.path)
                    .await
                    .unwrap();
            assert_eq!(
                target,
                ReviewTarget::Range {
                    from: base.clone(),
                    to: second.clone(),
                    commits: vec![first.clone(), second.clone()],
                }
            );
            validate_target(&target, 10, &repo.path).await.unwrap();
            let input = ReviewInput::collect(
                &target,
                crate::context::ReviewContext::default(),
                &crate::extract::Extractor::new(repo.path.clone()),
            )
            .await
            .unwrap();
            assert_eq!(input.head, second);
            assert_eq!(
                input
                    .commits
                    .iter()
                    .map(|commit| commit.hash.clone())
                    .collect::<Vec<_>>(),
                [first.clone(), second.clone()]
            );
            assert!(input.cumulative_diff.contains("first.txt"));
            assert!(input.cumulative_diff.contains("second.txt"));
            assert!(!input.cumulative_diff.contains("unrelated.txt"));
        }
    }

    #[tokio::test]
    async fn resolves_a_single_pull_request_commit_independently_of_head() {
        let repo = Repo::new().await;
        repo.commit("base.txt", "base").await;
        let commit = repo.commit("pr.txt", "PR commit").await;
        repo.commit("unrelated.txt", "unrelated commit").await;
        let target = resolve_pull_request_target(vec![commit.clone()], 10, &repo.path)
            .await
            .unwrap();
        assert_eq!(target, ReviewTarget::Commit(commit));
        validate_target(&target, 10, &repo.path).await.unwrap();
    }

    #[tokio::test]
    async fn rejects_empty_or_oversized_pull_requests_before_reading_git() {
        let directory = tempfile::tempdir().unwrap();
        assert_matches!(
            resolve_pull_request_target(vec![], 10, directory.path()).await,
            Err(ReviewTargetError::EmptyRange(_))
        );
        assert_matches!(
            resolve_pull_request_target(
                vec![
                    CommitHash::new("abc1234").unwrap(),
                    CommitHash::new("def5678").unwrap()
                ],
                1,
                directory.path()
            )
            .await,
            Err(ReviewTargetError::TooManyCommits {
                actual: 2,
                maximum: 1
            })
        );
    }

    #[tokio::test]
    async fn rejects_pull_request_commits_missing_locally() {
        let repo = Repo::new().await;
        repo.commit("base.txt", "base").await;
        assert_matches!(
            resolve_pull_request_target(vec![CommitHash::new("abc1234").unwrap()], 10, &repo.path)
                .await,
            Err(ReviewTargetError::Git(GitError::InvalidRevision(_)))
        );
    }

    #[tokio::test]
    async fn rejects_first_pull_request_commit_missing_locally() {
        let repo = Repo::new().await;
        repo.commit("base.txt", "base").await;
        let last = repo.commit("last.txt", "last PR commit").await;
        let missing = CommitHash::new("2222222222222222222222222222222222222222").unwrap();

        assert_matches!(
            resolve_pull_request_target(vec![missing.clone(), last], 10, &repo.path).await,
            Err(ReviewTargetError::Git(GitError::InvalidRevision(revision)))
                if revision == missing.as_ref()
        );
    }

    #[tokio::test]
    async fn rejects_last_pull_request_commit_missing_locally() {
        let repo = Repo::new().await;
        repo.commit("base.txt", "base").await;
        let first = repo.commit("first.txt", "first PR commit").await;
        let missing = CommitHash::new("0000000000000000000000000000000000000000").unwrap();

        assert_matches!(
            resolve_pull_request_target(vec![first, missing.clone()], 10, &repo.path).await,
            Err(ReviewTargetError::Git(GitError::InvalidRevision(revision)))
                if revision == missing.as_ref()
        );
    }

    #[tokio::test]
    async fn rejects_middle_pull_request_commit_missing_locally() {
        let repo = Repo::new().await;
        repo.commit("base.txt", "base").await;
        let first = repo.commit("first.txt", "first PR commit").await;
        let last = repo.commit("last.txt", "last PR commit").await;
        let missing = CommitHash::new("1111111111111111111111111111111111111111").unwrap();

        assert_matches!(
            resolve_pull_request_target(vec![first, missing.clone(), last], 10, &repo.path).await,
            Err(ReviewTargetError::Git(GitError::InvalidRevision(revision)))
                if revision == missing.as_ref()
        );
    }

    #[tokio::test]
    async fn pull_request_merge_commits_are_rejected() {
        let repo = Repo::new().await;
        let base = repo.commit("base.txt", "base").await;
        let first = repo.commit("pr.txt", "PR commit").await;
        run_git(&["checkout", "-b", "other", base.as_ref()], &repo.path)
            .await
            .unwrap();
        repo.commit("other.txt", "other commit").await;
        run_git(
            &[
                "merge",
                "--no-ff",
                "--no-edit",
                "--no-gpg-sign",
                first.as_ref(),
            ],
            &repo.path,
        )
        .await
        .unwrap();
        let merge = CommitHash::resolve("HEAD", &repo.path).await.unwrap();
        let target = resolve_pull_request_target(vec![first, merge.clone()], 10, &repo.path)
            .await
            .unwrap();
        assert_matches!(
            validate_target(&target, 10, &repo.path).await,
            Err(ReviewTargetError::MergeCommit(commit)) if commit == merge
        );
    }

    #[tokio::test]
    async fn resolves_range_oldest_to_newest() {
        let repo = Repo::new().await;
        let base = repo.commit("a.txt", "first").await;
        let second = repo.commit("b.txt", "second").await;
        let third = repo.commit("c.txt", "third").await;
        let target = format!("{base}..HEAD");

        assert_eq!(
            resolve_target(&target, 10, &repo.path).await.unwrap(),
            ReviewTarget::Range {
                from: base,
                to: third.clone(),
                commits: vec![second, third],
            }
        );
    }

    #[tokio::test]
    async fn rejects_invalid_and_empty_ranges() {
        let repo = Repo::new().await;
        repo.commit("a.txt", "first").await;

        assert_matches!(
            resolve_target("HEAD...HEAD", 10, &repo.path).await,
            Err(ReviewTargetError::InvalidRange(_))
        );
        assert_matches!(
            resolve_target("HEAD..HEAD", 10, &repo.path).await,
            Err(ReviewTargetError::EmptyRange(_))
        );
    }

    #[tokio::test]
    async fn rejects_oversized_ranges_while_resolving() {
        let repo = Repo::new().await;
        let base = repo.commit("a.txt", "first").await;
        repo.commit("b.txt", "second").await;
        repo.commit("c.txt", "third").await;
        let target = format!("{base}..HEAD");

        assert_matches!(
            resolve_target(&target, 1, &repo.path).await,
            Err(ReviewTargetError::TooManyCommits {
                actual: 2,
                maximum: 1
            })
        );
    }

    #[tokio::test]
    async fn enforces_maximum_commit_count() {
        let repo = Repo::new().await;
        let first = repo.commit("a.txt", "first").await;
        let second = repo.commit("b.txt", "second").await;
        let target = ReviewTarget::Range {
            from: first.clone(),
            to: second.clone(),
            commits: vec![first, second],
        };

        assert_matches!(
            validate_target(&target, 1, &repo.path).await,
            Err(ReviewTargetError::TooManyCommits {
                actual: 2,
                maximum: 1
            })
        );
    }
}
