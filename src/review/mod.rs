use std::fmt;
use std::path::Path;

use crate::console::Console;
use crate::git::{CommitHash, GitError, run_git};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewTarget {
    Commit(CommitHash),
    Range {
        revision: String,
        commits: Vec<CommitHash>,
    },
}

#[cfg_attr(not(test), expect(dead_code))]
pub async fn resolve_target(
    target: &str,
    project_root: &Path,
    console: Console,
) -> Result<ReviewTarget, ReviewTargetError> {
    if !target.contains("..") {
        return Ok(ReviewTarget::Commit(
            CommitHash::resolve(target, project_root, console).await?,
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
    let from = CommitHash::resolve(from, project_root, console).await?;
    let to = CommitHash::resolve(to, project_root, console).await?;
    let revision = format!("{from}..{to}");
    let output = run_git(&["rev-list", "--reverse", &revision], project_root, console).await?;
    let commits = output
        .lines()
        .filter(|line| !line.is_empty())
        .map(CommitHash::new)
        .collect::<Result<Vec<_>, _>>()?;
    if commits.is_empty() {
        return Err(ReviewTargetError::EmptyRange(target.to_string()));
    }

    Ok(ReviewTarget::Range { revision, commits })
}

#[cfg_attr(not(test), expect(dead_code))]
pub async fn validate_target(
    target: &ReviewTarget,
    max_commits: u32,
    project_root: &Path,
    console: Console,
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
            console,
        )
        .await?;
        if output.split_whitespace().count() > 2 {
            return Err(ReviewTargetError::MergeCommit(commit.clone()));
        }
    }

    Ok(())
}

#[derive(Debug)]
pub enum ReviewTargetError {
    Git(GitError),
    InvalidRange(String),
    EmptyRange(String),
    TooManyCommits { actual: usize, maximum: u32 },
    MergeCommit(CommitHash),
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
                    "review target contains {actual} commits (max: {maximum})"
                )
            }
            Self::MergeCommit(commit) => write!(f, "review target contains merge commit {commit}"),
        }
    }
}

impl std::error::Error for ReviewTargetError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Git(error) => Some(error),
            Self::InvalidRange(_) => None,
            Self::EmptyRange(_) => None,
            Self::TooManyCommits { .. } => None,
            Self::MergeCommit(_) => None,
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
            let console = Console::default();
            run_git(&["init"], &path, console).await.unwrap();
            run_git(
                &["config", "user.email", "test@example.com"],
                &path,
                console,
            )
            .await
            .unwrap();
            run_git(&["config", "user.name", "Test"], &path, console)
                .await
                .unwrap();
            Self { _tmp: tmp, path }
        }

        async fn commit(&self, file: &str, message: &str) -> CommitHash {
            std::fs::write(self.path.join(file), message).unwrap();
            run_git(&["add", file], &self.path, Console::default())
                .await
                .unwrap();
            run_git(
                &["commit", "--no-gpg-sign", "-m", message],
                &self.path,
                Console::default(),
            )
            .await
            .unwrap();
            CommitHash::resolve("HEAD", &self.path, Console::default())
                .await
                .unwrap()
        }
    }

    #[tokio::test]
    async fn resolves_range_oldest_to_newest() {
        let repo = Repo::new().await;
        let base = repo.commit("a.txt", "first").await;
        let second = repo.commit("b.txt", "second").await;
        let third = repo.commit("c.txt", "third").await;
        let target = format!("{base}..HEAD");

        assert_eq!(
            resolve_target(&target, &repo.path, Console::default())
                .await
                .unwrap(),
            ReviewTarget::Range {
                revision: format!("{base}..{third}"),
                commits: vec![second, third],
            }
        );
    }

    #[tokio::test]
    async fn rejects_invalid_and_empty_ranges() {
        let repo = Repo::new().await;
        repo.commit("a.txt", "first").await;

        assert_matches!(
            resolve_target("HEAD...HEAD", &repo.path, Console::default()).await,
            Err(ReviewTargetError::InvalidRange(_))
        );
        assert_matches!(
            resolve_target("HEAD..HEAD", &repo.path, Console::default()).await,
            Err(ReviewTargetError::EmptyRange(_))
        );
    }

    #[tokio::test]
    async fn enforces_maximum_commit_count() {
        let repo = Repo::new().await;
        let first = repo.commit("a.txt", "first").await;
        let second = repo.commit("b.txt", "second").await;
        let target = ReviewTarget::Range {
            revision: format!("{first}..{second}"),
            commits: vec![first, second],
        };

        assert_matches!(
            validate_target(&target, 1, &repo.path, Console::default()).await,
            Err(ReviewTargetError::TooManyCommits {
                actual: 2,
                maximum: 1
            })
        );
    }
}
