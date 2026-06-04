use std::path::Path;

use serde::{Deserialize, Serialize};

use super::ExtractError;
use crate::console::Console;
use crate::git::{CommitHash, GitError, run_git_bytes};

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FileContent {
    Text {
        path: String,
        hash: CommitHash,
        content: String,
    },
    Binary {
        path: String,
        hash: CommitHash,
        size: u64,
    },
}

pub async fn file_content(
    path: &Path,
    hash: CommitHash,
    project_root: &Path,
    console: Console,
) -> Result<FileContent, ExtractError> {
    let path = path.to_string_lossy().into_owned();
    let treeish = format!("{hash}:{path}");

    let bytes = run_git_bytes(&["show", &treeish], project_root, console).await?;

    if bytes.contains(&0u8) {
        return Ok(FileContent::Binary {
            path: path.clone(),
            hash,
            size: bytes.len() as u64,
        });
    }

    let content = String::from_utf8(bytes).map_err(GitError::FromUtf8)?;
    Ok(FileContent::Text {
        path,
        hash,
        content,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::console::Console;
    use crate::git::run_git;
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
    async fn file_content_returns_text_content() {
        let repo = Repo::new().await;
        let hash = repo.commit(&[("hello.txt", b"hello world")], "add").await;
        let result = file_content(Path::new("hello.txt"), hash, &repo.path, Console::default())
            .await
            .unwrap();
        let FileContent::Text { content, .. } = result else {
            panic!("expected text");
        };

        assert_eq!(content, "hello world");
    }

    #[tokio::test]
    async fn file_content_detects_binary() {
        let repo = Repo::new().await;
        let binary_data: Vec<u8> = vec![0x00, 0x01, 0x02, 0x03];
        let hash = repo
            .commit(&[("data.bin", &binary_data)], "add binary")
            .await;
        let result = file_content(Path::new("data.bin"), hash, &repo.path, Console::default())
            .await
            .unwrap();
        let FileContent::Binary { size, .. } = result else {
            panic!("expected binary");
        };

        assert_eq!(size, 4);
    }

    #[tokio::test]
    async fn file_content_reads_file_deleted_from_working_tree() {
        let repo = Repo::new().await;
        let hash = repo.commit(&[("f.txt", b"content")], "add").await;

        // delete from working tree but file still exists at `hash`
        std::fs::remove_file(repo.path.join("f.txt")).unwrap();

        let result = file_content(Path::new("f.txt"), hash, &repo.path, Console::default())
            .await
            .unwrap();
        let FileContent::Text { content, .. } = result else {
            panic!("expected text");
        };

        assert_eq!(content, "content");
    }

    #[tokio::test]
    async fn file_content_fails_for_nonexistent_path() {
        let repo = Repo::new().await;
        let hash = repo.commit(&[("a.txt", b"a")], "add").await;
        let err = file_content(
            Path::new("nonexistent.txt"),
            hash,
            &repo.path,
            Console::default(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ExtractError::Git { .. }));
    }
}
