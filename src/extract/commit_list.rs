use serde::{Deserialize, Serialize};

use crate::git::{CommitHash, run_git};

use super::{ExtractError, Extractor};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommitList {
    pub range: String,
    pub from: CommitHash,
    pub to: CommitHash,
    pub commits: Vec<CommitHash>,
}

impl Extractor {
    pub async fn commit_list(&self, range: &str) -> Result<CommitList, ExtractError> {
        self.debug(format_args!("extract commit list: {range}"));
        if range.contains("...") || !range.contains("..") {
            return Err(ExtractError::InvalidTwoDotRange(range.to_string()));
        }

        let (from, to) = range.split_once("..").unwrap();
        if from.is_empty() || to.is_empty() {
            return Err(ExtractError::InvalidTwoDotRange(range.to_string()));
        }

        let from = CommitHash::resolve(from, &self.project_root, self.console).await?;
        let to = CommitHash::resolve(to, &self.project_root, self.console).await?;
        let resolved_range = format!("{from}..{to}");

        let output = run_git(
            &["rev-list", "--reverse", &resolved_range],
            &self.project_root,
            self.console,
        )
        .await?;

        let commits = output
            .lines()
            .filter(|l| !l.is_empty())
            .map(CommitHash::new)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(CommitList {
            range: range.to_string(),
            from,
            to,
            commits,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::GitError;
    use std::assert_matches;
    use tempfile::TempDir;

    use crate::console::Console;

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
        let result = Extractor::new(repo.path.clone(), Console::default())
            .commit_list(&range)
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
        let result = Extractor::new(repo.path.clone(), Console::default())
            .commit_list(&range)
            .await
            .unwrap();

        assert_eq!(result.range, range);
    }

    #[tokio::test]
    async fn commit_list_round_trips_through_json() {
        let repo = Repo::new().await;
        let hash1 = repo.commit("a.txt", "first").await;
        repo.commit("b.txt", "second").await;

        let result = Extractor::new(repo.path.clone(), Console::default())
            .commit_list(&format!("{hash1}..HEAD"))
            .await
            .unwrap();
        let serialized = serde_json::to_string(&result).unwrap();
        let deserialized: CommitList = serde_json::from_str(&serialized).unwrap();

        assert_eq!(deserialized, result);
    }

    #[tokio::test]
    async fn commit_list_fails_for_three_dots_range() {
        let repo = Repo::new().await;
        let hash1 = repo.commit("a.txt", "first").await;
        let range = format!("{hash1}...HEAD");
        let err = Extractor::new(repo.path.clone(), Console::default())
            .commit_list(&range)
            .await
            .unwrap_err();
        assert_matches!(err, ExtractError::InvalidTwoDotRange(value) if value == range);
    }

    #[tokio::test]
    async fn commit_list_fails_for_non_range() {
        let repo = Repo::new().await;
        let hash1 = repo.commit("a.txt", "first").await;
        let err = Extractor::new(repo.path.clone(), Console::default())
            .commit_list(hash1.as_ref())
            .await
            .unwrap_err();
        assert_matches!(err, ExtractError::InvalidTwoDotRange(value) if value == hash1.as_ref());
    }

    #[tokio::test]
    async fn commit_list_fails_for_missing_from_revision() {
        let repo = Repo::new().await;
        repo.commit("a.txt", "first").await;
        let from = "deadbeef1234567";
        let range = format!("{from}..HEAD");
        let err = Extractor::new(repo.path.clone(), Console::default())
            .commit_list(&range)
            .await
            .unwrap_err();
        assert_matches!(err, ExtractError::Git(GitError::InvalidRevision(value)) if value == from);
    }

    #[tokio::test]
    async fn commit_list_fails_for_missing_to_revision() {
        let repo = Repo::new().await;
        let hash1 = repo.commit("a.txt", "first").await;
        let to = "deadbeef1234567";
        let range = format!("{hash1}..{to}");
        let err = Extractor::new(repo.path.clone(), Console::default())
            .commit_list(&range)
            .await
            .unwrap_err();
        assert_matches!(err, ExtractError::Git(GitError::InvalidRevision(value)) if value == to);
    }
}
