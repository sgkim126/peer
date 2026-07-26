use std::path::Path;

use serde::{Deserialize, Deserializer, Serialize, de};

use crate::git::{CommitHash, GitError, run_git_bytes};

use super::{ExtractError, Extractor, validate_repository_relative_path};

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FileContent {
    Text {
        path: String,
        hash: CommitHash,
        content: String,
        #[serde(flatten, skip_serializing_if = "Option::is_none")]
        range: Option<FileContentRange>,
    },
    Binary {
        path: String,
        hash: CommitHash,
        size: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FileContentRange {
    start_line: u32,
    end_line: u32,
}

impl<'de> Deserialize<'de> for FileContentRange {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawFileContentRange {
            start_line: u32,
            end_line: u32,
        }

        let raw = RawFileContentRange::deserialize(deserializer)?;
        Self::new(raw.start_line, raw.end_line).map_err(de::Error::custom)
    }
}

impl FileContentRange {
    pub fn new(start_line: u32, end_line: u32) -> Result<Self, ExtractError> {
        if start_line == 0 {
            return Err(ExtractError::InvalidFileContentRange(
                "start_line must be at least 1".to_string(),
            ));
        }
        if end_line < start_line {
            return Err(ExtractError::InvalidFileContentRange(format!(
                "end_line({end_line}) must not be before start_line({start_line})"
            )));
        }
        Ok(Self {
            start_line,
            end_line,
        })
    }

    pub fn start_line(self) -> u32 {
        self.start_line
    }

    pub fn end_line(self) -> u32 {
        self.end_line
    }
}

impl Extractor {
    pub async fn file_content(
        &self,
        revision: &str,
        path: &Path,
        line_range: Option<FileContentRange>,
    ) -> Result<FileContent, ExtractError> {
        self.debug(format_args!(
            "extract file content: {revision} {}",
            path.display()
        ));
        validate_repository_relative_path(path)?;
        let hash = CommitHash::resolve(revision, &self.project_root, self.console).await?;
        // TODO: Normalize Windows path separators to `/` for Git tree paths.
        let path = path.to_string_lossy().into_owned();
        let treeish = format!("{hash}:{path}");

        let bytes = run_git_bytes(&["show", &treeish], &self.project_root, self.console).await?;

        if bytes.contains(&0u8) {
            if line_range.is_some() {
                return Err(ExtractError::InvalidFileContentRange(
                    "a binary file doesn't have lines".to_string(),
                ));
            }
            return Ok(FileContent::Binary {
                path: path.clone(),
                hash,
                size: bytes.len() as u64,
            });
        }

        let content = String::from_utf8(bytes).map_err(GitError::FromUtf8)?;
        let (content, range) = select_line_range(&content, line_range)?;
        Ok(FileContent::Text {
            path,
            hash,
            content,
            range,
        })
    }
}

fn select_line_range(
    content: &str,
    line_range: Option<FileContentRange>,
) -> Result<(String, Option<FileContentRange>), ExtractError> {
    let Some(line_range) = line_range else {
        return Ok((content.to_string(), None));
    };

    let lines = content.split_inclusive('\n').collect::<Vec<_>>();
    let start_index = (line_range.start_line - 1) as usize;
    if start_index >= lines.len() {
        return Err(ExtractError::InvalidFileContentRange(format!(
            "start_line {} is beyond the end of the file",
            line_range.start_line
        )));
    }

    let end_line = line_range.end_line.min(lines.len() as u32);
    let end_index = (end_line - 1) as usize;
    let content = lines[start_index..=end_index].concat();
    Ok((
        content,
        Some(FileContentRange {
            start_line: line_range.start_line,
            end_line,
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::assert_matches;

    use tempfile::TempDir;

    use crate::console::Console;
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
        let result = Extractor::new(repo.path.clone(), Console::default())
            .file_content(hash.as_ref(), Path::new("hello.txt"), None)
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
        let result = Extractor::new(repo.path.clone(), Console::default())
            .file_content(hash.as_ref(), Path::new("data.bin"), None)
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

        let result = Extractor::new(repo.path.clone(), Console::default())
            .file_content(hash.as_ref(), Path::new("f.txt"), None)
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
        let err = Extractor::new(repo.path.clone(), Console::default())
            .file_content(hash.as_ref(), Path::new("nonexistent.txt"), None)
            .await
            .unwrap_err();
        assert_matches!(err, ExtractError::Git { .. });
    }

    #[tokio::test]
    async fn file_content_rejects_absolute_and_parent_paths() {
        let extractor = Extractor::new(std::path::PathBuf::from("/unused"), Console::default());

        for path in [Path::new("/tmp/file.rs"), Path::new("src/../file.rs")] {
            let error = extractor
                .file_content("HEAD", path, None)
                .await
                .unwrap_err();

            assert_matches!(error, ExtractError::InvalidRepositoryRelativePath(_));
        }
    }

    #[tokio::test]
    async fn file_content_returns_requested_line_range() {
        let repo = Repo::new().await;
        let hash = repo
            .commit(&[("lines.txt", b"one\ntwo\nthree\nfour\n")], "add lines")
            .await;

        let result = Extractor::new(repo.path.clone(), Console::default())
            .file_content(
                hash.as_ref(),
                Path::new("lines.txt"),
                Some(FileContentRange::new(2, 10).unwrap()),
            )
            .await
            .unwrap();
        let FileContent::Text { content, range, .. } = result else {
            panic!("expected text");
        };

        assert_eq!(content, "two\nthree\nfour\n");
        assert_eq!(range, Some(FileContentRange::new(2, 4).unwrap()));
    }

    #[tokio::test]
    async fn file_content_rejects_line_range_for_binary_file() {
        let repo = Repo::new().await;
        let hash = repo
            .commit(&[("data.bin", b"\0binary")], "add binary")
            .await;

        let error = Extractor::new(repo.path.clone(), Console::default())
            .file_content(
                hash.as_ref(),
                Path::new("data.bin"),
                Some(FileContentRange::new(1, 1).unwrap()),
            )
            .await
            .unwrap_err();

        assert_matches!(error, ExtractError::InvalidFileContentRange(_));
    }

    #[test]
    fn file_content_range_rejects_invalid_line_ranges() {
        assert_matches!(
            FileContentRange::new(0, 1),
            Err(ExtractError::InvalidFileContentRange(_))
        );
        assert_matches!(
            FileContentRange::new(2, 1),
            Err(ExtractError::InvalidFileContentRange(_))
        );
    }

    #[test]
    fn file_content_range_deserialization_rejects_invalid_line_ranges() {
        for value in [
            serde_json::json!({ "start_line": 0, "end_line": 1 }),
            serde_json::json!({ "start_line": 1, "end_line": 0 }),
            serde_json::json!({ "start_line": 2, "end_line": 1 }),
        ] {
            assert!(serde_json::from_value::<FileContentRange>(value).is_err());
        }
    }
}
