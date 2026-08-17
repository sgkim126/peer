use log::trace;
use serde::{Deserialize, Serialize};

use crate::git::{CommitHash, run_git};

use super::{ExtractError, Extractor};

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct CommitDiff {
    pub hash: CommitHash,
    pub diff: String,
}

impl Extractor {
    pub async fn commit_diff(&self, revision: &str) -> Result<CommitDiff, ExtractError> {
        trace!("extract commit diff: {revision:?}");
        let hash = CommitHash::resolve(revision, &self.project_root).await?;

        let diff = run_git(
            &[
                "diff-tree",
                "--no-commit-id",
                "--root",
                "-r",
                "-p",
                hash.as_ref(),
            ],
            &self.project_root,
        )
        .await?;

        Ok(CommitDiff { hash, diff })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::assert_matches;

    use tempfile::TempDir;

    struct Repo {
        _tmp: TempDir,
        path: std::path::PathBuf,
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

        async fn commit(&self, files: &[(&str, &[u8])], message: &str) -> CommitHash {
            for (name, content) in files {
                std::fs::write(self.path.join(name), content).unwrap();
                run_git(&["add", name], &self.path).await.unwrap();
            }
            run_git(&["commit", "--no-gpg-sign", "-m", message], &self.path)
                .await
                .unwrap();
            let raw = run_git(&["rev-parse", "HEAD"], &self.path).await.unwrap();
            CommitHash::new(raw.trim()).unwrap()
        }
    }

    #[tokio::test]
    async fn commit_diff_contains_added_content() {
        let repo = Repo::new().await;
        let hash = repo
            .commit(&[("hello.txt", b"hello world\n")], "add hello")
            .await;
        let result = Extractor::new(repo.path.clone())
            .commit_diff(hash.as_ref())
            .await
            .unwrap();
        assert!(result.diff.contains("+hello world"));
    }

    #[tokio::test]
    async fn commit_diff_contains_modified_content() {
        let repo = Repo::new().await;
        repo.commit(&[("f.txt", b"old\n")], "initial").await;
        std::fs::write(repo.path.join("f.txt"), b"new\n").unwrap();
        run_git(&["add", "f.txt"], &repo.path).await.unwrap();
        run_git(&["commit", "--no-gpg-sign", "-m", "modify"], &repo.path)
            .await
            .unwrap();
        let raw = run_git(&["rev-parse", "HEAD"], &repo.path).await.unwrap();
        let hash = CommitHash::new(raw.trim()).unwrap();

        let result = Extractor::new(repo.path.clone())
            .commit_diff(hash.as_ref())
            .await
            .unwrap();

        assert!(result.diff.contains("-old"));
        assert!(result.diff.contains("+new"));
    }

    #[tokio::test]
    async fn commit_diff_fails_for_unknown_hash() {
        let tmp = tempfile::tempdir().unwrap();
        run_git(&["init"], tmp.path()).await.unwrap();
        let hash = "deadbeef";
        let err = Extractor::new(tmp.path().to_path_buf())
            .commit_diff(hash)
            .await
            .unwrap_err();

        assert_matches!(err, ExtractError::Git { .. });
    }
}
