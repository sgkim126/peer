use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::console::Console;
use crate::git::{CommitHash, run_git};

use super::ExtractError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommitList {
    pub range: String,
    pub commits: Vec<CommitHash>,
}

pub async fn commit_list(
    range: &str,
    project_root: &Path,
    console: Console,
) -> Result<CommitList, ExtractError> {
    if range.contains("...") || !range.contains("..") {
        return Err(ExtractError::InvalidRange(range.to_string()));
    }

    let (from, to) = range.split_once("..").unwrap();
    if from.is_empty() || to.is_empty() {
        return Err(ExtractError::InvalidRange(range.to_string()));
    }

    CommitHash::resolve(from, project_root, console)
        .await
        .map_err(|_| ExtractError::InvalidRevision(from.to_string()))?;
    CommitHash::resolve(to, project_root, console)
        .await
        .map_err(|_| ExtractError::InvalidRevision(to.to_string()))?;

    let output = run_git(&["rev-list", "--reverse", range], project_root, console).await?;

    let commits = output
        .lines()
        .filter(|l| !l.is_empty())
        .map(CommitHash::new)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(CommitList {
        range: range.to_string(),
        commits,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

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
            std::fs::write(self.path.join(file), file).unwrap();
            run_git(&["add", file], &self.path, console).await.unwrap();
            run_git(
                &["commit", "--no-gpg-sign", "-m", message],
                &self.path,
                console,
            )
            .await
            .unwrap();
            let raw = run_git(&["rev-parse", "HEAD"], &self.path, console)
                .await
                .unwrap();
            CommitHash::new(raw.trim()).unwrap()
        }
    }

    #[tokio::test]
    async fn commit_list_returns_oldest_to_newest() {
        let repo = Repo::new().await;
        let hash1 = repo.commit("a.txt", "first").await;
        let hash2 = repo.commit("b.txt", "second").await;
        let hash3 = repo.commit("c.txt", "third").await;

        let range = format!("{hash1}..HEAD");
        let result = commit_list(&range, &repo.path, Console::default())
            .await
            .unwrap();

        assert_eq!(result.commits, vec![hash2, hash3]);
    }

    #[tokio::test]
    async fn commit_list_range_is_preserved() {
        let repo = Repo::new().await;
        let hash1 = repo.commit("a.txt", "first").await;
        repo.commit("b.txt", "second").await;

        let range = format!("{hash1}..HEAD");
        let result = commit_list(&range, &repo.path, Console::default())
            .await
            .unwrap();

        assert_eq!(result.range, range);
    }

    #[tokio::test]
    async fn commit_list_fails_for_three_dots_range() {
        let repo = Repo::new().await;
        let hash1 = repo.commit("a.txt", "first").await;
        let range = format!("{hash1}...HEAD");
        let err = commit_list(&range, &repo.path, Console::default())
            .await
            .unwrap_err();
        assert!(matches!(err, ExtractError::InvalidRange(value) if value == range));
    }

    #[tokio::test]
    async fn commit_list_fails_for_non_range() {
        let repo = Repo::new().await;
        let hash1 = repo.commit("a.txt", "first").await;
        let err = commit_list(hash1.as_ref(), &repo.path, Console::default())
            .await
            .unwrap_err();
        assert!(matches!(err, ExtractError::InvalidRange(value) if value == hash1.as_ref()));
    }

    #[tokio::test]
    async fn commit_list_fails_for_missing_from_revision() {
        let repo = Repo::new().await;
        repo.commit("a.txt", "first").await;
        let from = "deadbeef1234567";
        let range = format!("{from}..HEAD");
        let err = commit_list(&range, &repo.path, Console::default())
            .await
            .unwrap_err();
        assert!(matches!(err, ExtractError::InvalidRevision(value) if value == from));
    }

    #[tokio::test]
    async fn commit_list_fails_for_missing_to_revision() {
        let repo = Repo::new().await;
        let hash1 = repo.commit("a.txt", "first").await;
        let to = "deadbeef1234567";
        let range = format!("{hash1}..{to}");
        let err = commit_list(&range, &repo.path, Console::default())
            .await
            .unwrap_err();
        assert!(matches!(err, ExtractError::InvalidRevision(value) if value == to));
    }
}
