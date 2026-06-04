use std::path::Path;

use serde::{Deserialize, Serialize};

use super::ExtractError;
use crate::console::Console;
use crate::git::{CommitHash, run_git};

#[derive(Debug, PartialEq, Deserialize, Serialize)]
pub struct CommitMessage {
    pub hash: CommitHash,
    pub message: String,
}

pub async fn commit_message(
    hash: CommitHash,
    project_root: &Path,
    console: Console,
) -> Result<CommitMessage, ExtractError> {
    let output = run_git(
        &["log", "-1", "--format=%B", hash.as_ref()],
        project_root,
        console,
    )
    .await?;

    Ok(CommitMessage {
        hash,
        message: output.trim_end().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::TempDir;

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
        let result = commit_message(hash.clone(), &repo.path, console)
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
        let result = commit_message(hash, &repo.path, console).await.unwrap();
        assert_eq!(result.message, msg);
    }

    #[tokio::test]
    async fn commit_message_fails_for_unknown_hash() {
        let console = Console::default();
        let tmp = tempfile::tempdir().unwrap();
        run_git(&["init"], tmp.path(), console).await.unwrap();
        let hash = CommitHash::new("deadbeef").unwrap();
        let err = commit_message(hash, tmp.path(), console).await.unwrap_err();
        assert!(matches!(err, ExtractError::Git { .. }));
    }
}
