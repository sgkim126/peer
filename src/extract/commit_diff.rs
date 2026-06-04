use serde::{Deserialize, Serialize};

use super::{ExtractError, Extractor};
use crate::git::{CommitHash, run_git};

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct CommitDiff {
    pub hash: CommitHash,
    pub diff: String,
}

impl Extractor {
    pub async fn commit_diff(&self, revision: &str) -> Result<CommitDiff, ExtractError> {
        let hash = CommitHash::resolve(revision, &self.project_root, self.console).await?;

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
            self.console,
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

        async fn commit(&self, files: &[(&str, &[u8])], message: &str) -> CommitHash {
            let console = Console::default();
            for (name, content) in files {
                std::fs::write(self.path.join(name), content).unwrap();
                run_git(&["add", name], &self.path, console).await.unwrap();
            }
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
    async fn commit_diff_contains_added_content() {
        let repo = Repo::new().await;
        let hash = repo
            .commit(&[("hello.txt", b"hello world\n")], "add hello")
            .await;
        let result = Extractor::new(repo.path.clone(), Console::default())
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
        let console = Console::default();
        run_git(&["add", "f.txt"], &repo.path, console)
            .await
            .unwrap();
        run_git(
            &["commit", "--no-gpg-sign", "-m", "modify"],
            &repo.path,
            console,
        )
        .await
        .unwrap();
        let raw = run_git(&["rev-parse", "HEAD"], &repo.path, console)
            .await
            .unwrap();
        let hash = CommitHash::new(raw.trim()).unwrap();

        let result = Extractor::new(repo.path.clone(), Console::default())
            .commit_diff(hash.as_ref())
            .await
            .unwrap();

        assert!(result.diff.contains("-old"));
        assert!(result.diff.contains("+new"));
    }

    #[tokio::test]
    async fn commit_diff_fails_for_unknown_hash() {
        let tmp = tempfile::tempdir().unwrap();
        run_git(&["init"], tmp.path(), Console::default())
            .await
            .unwrap();
        let hash = "deadbeef";
        let err = Extractor::new(tmp.path().to_path_buf(), Console::default())
            .commit_diff(hash)
            .await
            .unwrap_err();

        assert_matches!(err, ExtractError::Git { .. });
    }
}
