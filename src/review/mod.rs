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

#[allow(dead_code)]
pub async fn resolve_target(
    target: &str,
    project_root: &Path,
    console: Console,
) -> Result<ReviewTarget, ReviewTargetError> {
    if !target.contains("..") {
        let commit = CommitHash::resolve(target, project_root, console).await?;

        return Ok(ReviewTarget::Commit(commit));
    }

    if target.contains("...") {
        return Err(ReviewTargetError::InvalidRange(target.to_string()));
    }
    let Some((from, to)) = target.split_once("..") else {
        return Err(ReviewTargetError::InvalidRange(target.to_string()));
    };
    if from.is_empty() || to.is_empty() {
        return Err(ReviewTargetError::InvalidRange(target.to_string()));
    }

    let output = run_git(&["rev-list", "--reverse", target], project_root, console).await?;
    let commits = output
        .lines()
        .filter(|line| !line.is_empty())
        .map(CommitHash::new)
        .collect::<Result<Vec<_>, _>>()?;
    if commits.is_empty() {
        return Err(ReviewTargetError::EmptyRange(target.to_string()));
    }

    Ok(ReviewTarget::Range {
        revision: target.to_string(),
        commits,
    })
}

#[derive(Debug)]
pub enum ReviewTargetError {
    Git(GitError),
    InvalidRange(String),
    EmptyRange(String),
}

impl fmt::Display for ReviewTargetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Git(error) => error.fmt(f),
            Self::InvalidRange(range) => write!(f, "{range} is not a two-dots range"),
            Self::EmptyRange(range) => write!(f, "{range} contains no commits"),
        }
    }
}

impl std::error::Error for ReviewTargetError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Git(error) => Some(error),
            Self::InvalidRange(_) => None,
            Self::EmptyRange(_) => None,
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
    use tempfile::TempDir;

    use super::*;
    use crate::git::run_git;

    struct Repo {
        _tmp: TempDir,
        path: std::path::PathBuf,
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
            let console = Console::default();
            std::fs::write(self.path.join(file), message).unwrap();
            run_git(&["add", file], &self.path, console).await.unwrap();
            run_git(
                &["commit", "--no-gpg-sign", "-m", message],
                &self.path,
                console,
            )
            .await
            .unwrap();
            let hash = run_git(&["rev-parse", "HEAD"], &self.path, console)
                .await
                .unwrap();

            CommitHash::new(hash.trim()).unwrap()
        }
    }

    #[tokio::test]
    async fn resolves_single_revision_to_tip_commit() {
        let repo = Repo::new().await;
        let expected = repo.commit("a.txt", "first").await;

        let target = resolve_target("HEAD", &repo.path, Console::default())
            .await
            .unwrap();

        assert_eq!(target, ReviewTarget::Commit(expected));
    }

    #[tokio::test]
    async fn resolves_range_oldest_to_newest() {
        let repo = Repo::new().await;
        let base = repo.commit("a.txt", "first").await;
        let second = repo.commit("b.txt", "second").await;
        let third = repo.commit("c.txt", "third").await;
        let revision = format!("{base}..HEAD");

        let target = resolve_target(&revision, &repo.path, Console::default())
            .await
            .unwrap();

        assert_eq!(
            target,
            ReviewTarget::Range {
                revision,
                commits: vec![second, third],
            }
        );
    }

    #[tokio::test]
    async fn rejects_three_dot_range() {
        let repo = Repo::new().await;
        repo.commit("a.txt", "first").await;
        let revision = "HEAD...HEAD";

        let error = resolve_target(revision, &repo.path, Console::default())
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            ReviewTargetError::InvalidRange(value) if value == revision
        ));
    }

    #[tokio::test]
    async fn rejects_empty_range() {
        let repo = Repo::new().await;
        repo.commit("a.txt", "first").await;
        let revision = "HEAD..HEAD";

        let error = resolve_target(revision, &repo.path, Console::default())
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            ReviewTargetError::EmptyRange(value) if value == revision
        ));
    }

    #[tokio::test]
    async fn rejects_invalid_single_revision() {
        let repo = Repo::new().await;

        let error = resolve_target("missing", &repo.path, Console::default())
            .await
            .unwrap_err();

        assert!(matches!(error, ReviewTargetError::Git(_)));
    }
}
