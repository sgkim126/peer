use serde::{Deserialize, Serialize};

use crate::git::{CommitHash, run_git};

use super::{ExtractError, Extractor};

#[derive(Debug, PartialEq, Deserialize, Serialize)]
pub struct CommitMessage {
    pub hash: CommitHash,
    pub message: String,
}

impl Extractor {
    pub async fn commit_message(&self, revision: &str) -> Result<CommitMessage, ExtractError> {
        let hash = CommitHash::resolve(revision, &self.project_root, self.console).await?;

        let output = run_git(
            &["log", "-1", "--format=%B", hash.as_ref()],
            &self.project_root,
            self.console,
        )
        .await?;

        Ok(CommitMessage {
            hash,
            message: output.trim_end().to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::assert_matches;
    use std::path::Path;
    use tempfile::TempDir;

    use crate::console::Console;

    async fn init_repo_with_commit(dir: &Path, message: &str) -> CommitHash {
        let console = Console::default();
        run_git(&["init"], dir, console).await.unwrap();
        run_git(&["config", "user.email", "test@example.com"], dir, console)
            .await
            .unwrap();
        run_git(&["config", "user.name", "Test"], dir, console)
            .await
            .unwrap();
        std::fs::write(dir.join("f"), "x").unwrap();
        run_git(&["add", "f"], dir, console).await.unwrap();
        run_git(&["commit", "--no-gpg-sign", "-m", message], dir, console)
            .await
            .unwrap();
        let raw = run_git(&["rev-parse", "HEAD"], dir, console).await.unwrap();
        CommitHash::new(raw.trim()).unwrap()
    }

    struct Repo {
        _tmp: TempDir,
        path: std::path::PathBuf,
    }

    impl Repo {
        async fn new(message: &str) -> (Self, CommitHash) {
            let tmp = tempfile::tempdir().unwrap();
            let hash = init_repo_with_commit(tmp.path(), message).await;
            let path = tmp.path().to_path_buf();
            (Self { _tmp: tmp, path }, hash)
        }
    }

    #[tokio::test]
    async fn commit_message_returns_correct_message() {
        let console = Console::default();
        let (repo, hash) = Repo::new("initial commit").await;
        let result = Extractor::new(repo.path.clone(), console)
            .commit_message(hash.as_ref())
            .await
            .unwrap();
        assert_eq!(result.hash, hash);
        assert_eq!(result.message, "initial commit");
    }

    #[tokio::test]
    async fn commit_message_preserves_multiline_message() {
        let console = Console::default();
        let msg = "subject line\n\nbody paragraph";
        let (repo, hash) = Repo::new(msg).await;
        let result = Extractor::new(repo.path.clone(), console)
            .commit_message(hash.as_ref())
            .await
            .unwrap();
        assert_eq!(result.message, msg);
    }

    #[tokio::test]
    async fn commit_message_fails_for_unknown_hash() {
        let console = Console::default();
        let tmp = tempfile::tempdir().unwrap();
        run_git(&["init"], tmp.path(), console).await.unwrap();
        let hash = "deadbeef";
        let err = Extractor::new(tmp.path().to_path_buf(), console)
            .commit_message(hash)
            .await
            .unwrap_err();
        assert_matches!(err, ExtractError::Git { .. });
    }
}
