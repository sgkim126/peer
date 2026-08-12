use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::console::Console;
use crate::git::{CommitHash, run_git};

use super::{ExtractError, Extractor};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub status: FileStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    pub is_binary: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommitFiles {
    pub hash: CommitHash,
    pub files: Vec<FileEntry>,
}

#[derive(Debug, PartialEq, Eq)]
struct RawFileEntry {
    status: FileStatus,
    path: String,
    source_path: Option<String>,
}

/// Parses `git diff-tree --name-status -z` output.
fn parse_name_status(output: &str, console: Console) -> Result<Vec<RawFileEntry>, ExtractError> {
    let mut entries = Vec::new();
    let mut fields = output.split('\0').filter(|field| !field.is_empty());
    while let Some(code) = fields.next() {
        // git appends a similarity score to R and C codes (e.g. R90, C80)
        let status = match code.chars().next() {
            Some('R') => FileStatus::Renamed,
            Some('C') => FileStatus::Copied,
            Some('A') => FileStatus::Added,
            Some('M') => FileStatus::Modified,
            Some('D') => FileStatus::Deleted,
            Some('T') => FileStatus::TypeChanged,
            _ => {
                let message = format!("unknown git name-status code: {code}");
                console.debug(format_args!("{message}; output: {output:?}"));
                return Err(ExtractError::MalformedGitOutput(message));
            }
        };
        let entry = match status {
            FileStatus::Renamed | FileStatus::Copied => {
                let operation = if status == FileStatus::Renamed {
                    "rename"
                } else {
                    "copy"
                };
                let Some(source_path) = fields.next() else {
                    let message = format!("incomplete git {operation} entry for status: {code}");
                    console.debug(format_args!("{message}; output: {output:?}"));
                    return Err(ExtractError::MalformedGitOutput(message));
                };
                let Some(path) = fields.next() else {
                    let message = format!("incomplete git {operation} entry for status: {code}");
                    console.debug(format_args!("{message}; output: {output:?}"));
                    return Err(ExtractError::MalformedGitOutput(message));
                };
                RawFileEntry {
                    status,
                    path: path.to_string(),
                    source_path: Some(source_path.to_string()),
                }
            }
            _ => {
                let Some(path) = fields.next() else {
                    let message = format!("incomplete git name-status entry for status: {code}");
                    console.debug(format_args!("{message}; output: {output:?}"));
                    return Err(ExtractError::MalformedGitOutput(message));
                };
                RawFileEntry {
                    status,
                    path: path.to_string(),
                    source_path: None,
                }
            }
        };
        entries.push(entry);
    }
    Ok(entries)
}

/// Parses `git diff-tree --numstat -z` output.
fn parse_binary_paths(numstat: &str, console: Console) -> Result<HashSet<&str>, ExtractError> {
    let mut binary = HashSet::new();
    for record in numstat.split('\0') {
        if record.is_empty() {
            continue;
        }
        let mut parts = record.splitn(3, '\t');
        let Some(added) = parts.next() else {
            let message = "missing added count in git numstat".to_string();
            console.debug(format_args!("{message}; output: {numstat:?}"));
            return Err(ExtractError::MalformedGitOutput(message));
        };
        let Some(deleted) = parts.next() else {
            let message = "missing deleted count in git numstat".to_string();
            console.debug(format_args!("{message}; output: {numstat:?}"));
            return Err(ExtractError::MalformedGitOutput(message));
        };
        let Some(path) = parts.next() else {
            let message = "missing path in git numstat".to_string();
            console.debug(format_args!("{message}; output: {numstat:?}"));
            return Err(ExtractError::MalformedGitOutput(message));
        };
        if added == "-" && deleted == "-" {
            binary.insert(path);
        }
    }
    Ok(binary)
}

impl Extractor {
    pub async fn commit_files(&self, revision: &str) -> Result<CommitFiles, ExtractError> {
        self.console
            .debug(format_args!("extract commit files: {revision}"));
        let hash = CommitHash::resolve(revision, &self.project_root, self.console).await?;

        let name_status_out = run_git(
            &[
                "diff-tree",
                "--no-commit-id",
                "--root",
                "-r",
                "--name-status",
                "-z",
                "-M",
                "-C",
                hash.as_ref(),
            ],
            &self.project_root,
            self.console,
        )
        .await?;

        // Intentionally omit -M/-C: binary renames must be reported as
        // separate old-path deletions and new-path additions so both paths
        // are detected.
        // Use -z to use NUL as delimiters so paths with tabs, quotes, or
        // non-ASCII characters are not C-quoted by Git.
        let numstat_out = run_git(
            &[
                "diff-tree",
                "--no-commit-id",
                "--root",
                "-r",
                "--numstat",
                "-z",
                hash.as_ref(),
            ],
            &self.project_root,
            self.console,
        )
        .await?;

        let binary_paths = parse_binary_paths(&numstat_out, self.console)?;
        let raw_entries = parse_name_status(&name_status_out, self.console)?;

        let files = raw_entries
            .into_iter()
            .map(|entry| {
                let is_binary = binary_paths.contains(entry.path.as_str())
                    || entry
                        .source_path
                        .as_deref()
                        .is_some_and(|p| binary_paths.contains(p));
                FileEntry {
                    path: entry.path,
                    status: entry.status,
                    source_path: entry.source_path,
                    is_binary,
                }
            })
            .collect();

        Ok(CommitFiles { hash, files })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::assert_matches;

    use tempfile::TempDir;

    #[test]
    fn parse_name_status_added() {
        let console = Console::default();

        let entries = parse_name_status("A\0src/new_file.rs\0", console).unwrap();
        assert_eq!(
            entries,
            vec![RawFileEntry {
                status: FileStatus::Added,
                path: "src/new_file.rs".into(),
                source_path: None,
            }]
        );
    }

    #[test]
    fn parse_name_status_modified() {
        let console = Console::default();

        let entries = parse_name_status("M\0src/lib.rs\0", console).unwrap();
        assert_eq!(
            entries,
            vec![RawFileEntry {
                status: FileStatus::Modified,
                path: "src/lib.rs".into(),
                source_path: None,
            }]
        );
    }

    #[test]
    fn parse_name_status_deleted() {
        let console = Console::default();

        let entries = parse_name_status("D\0src/old_file.rs\0", console).unwrap();
        assert_eq!(
            entries,
            vec![RawFileEntry {
                status: FileStatus::Deleted,
                path: "src/old_file.rs".into(),
                source_path: None,
            }]
        );
    }

    #[test]
    fn parse_name_status_renamed_includes_source_path() {
        let console = Console::default();

        let entries =
            parse_name_status("R90\0src/old_name.rs\0src/new_name.rs\0", console).unwrap();
        assert_eq!(
            entries,
            vec![RawFileEntry {
                status: FileStatus::Renamed,
                path: "src/new_name.rs".into(),
                source_path: Some("src/old_name.rs".into()),
            }]
        );
    }

    #[test]
    fn parse_name_status_copied_includes_source_path() {
        let console = Console::default();

        let entries = parse_name_status("C80\0src/original.rs\0src/copy.rs\0", console).unwrap();
        assert_eq!(
            entries,
            vec![RawFileEntry {
                status: FileStatus::Copied,
                path: "src/copy.rs".into(),
                source_path: Some("src/original.rs".into()),
            }]
        );
    }

    #[test]
    fn parse_name_status_type_changed() {
        let console = Console::default();

        let entries = parse_name_status("T\0src/some_file.rs\0", console).unwrap();
        assert_eq!(
            entries,
            vec![RawFileEntry {
                status: FileStatus::TypeChanged,
                path: "src/some_file.rs".into(),
                source_path: None,
            }]
        );
    }

    #[test]
    fn parse_name_status_multiple_entries() {
        let console = Console::default();

        let output = "A\0file1.txt\0M\0file2.rs\0D\0file3.old\0";
        let entries = parse_name_status(output, console).unwrap();
        assert_eq!(
            entries,
            vec![
                RawFileEntry {
                    status: FileStatus::Added,
                    path: "file1.txt".into(),
                    source_path: None,
                },
                RawFileEntry {
                    status: FileStatus::Modified,
                    path: "file2.rs".into(),
                    source_path: None,
                },
                RawFileEntry {
                    status: FileStatus::Deleted,
                    path: "file3.old".into(),
                    source_path: None,
                },
            ]
        );
    }

    #[test]
    fn parse_name_status_rejects_unknown_codes() {
        let console = Console::default();

        let output = "X\0unknown.txt\0A\0known.txt\0";
        assert_matches!(
            parse_name_status(output, console),
            Err(ExtractError::MalformedGitOutput(message)) if message.contains("unknown git name-status code")
        );
    }

    #[test]
    fn parse_name_status_empty_input() {
        let console = Console::default();

        assert_eq!(
            parse_name_status("", console).unwrap(),
            Vec::<RawFileEntry>::new()
        );
    }

    #[test]
    fn parse_name_status_preserves_special_paths() {
        let console = Console::default();

        let entries = parse_name_status("A\0café\tfile.txt\0", console).unwrap();

        assert_eq!(
            entries,
            vec![RawFileEntry {
                status: FileStatus::Added,
                path: "café\tfile.txt".into(),
                source_path: None,
            }]
        );
    }

    #[test]
    fn parse_binary_paths_detects_binary() {
        let binary1 = "image.png";
        let non_binary1 = "file.txt";
        let binary2 = "archive.zip";
        let numstat = format!("-\t-\t{binary1}\x005\t3\t{non_binary1}\x00-\t-\t{binary2}\0");
        let binary = parse_binary_paths(&numstat, Console::default()).unwrap();
        assert!(binary.contains(binary1));
        assert!(binary.contains(binary2));
        assert!(!binary.contains(non_binary1));
    }

    #[test]
    fn parse_binary_paths_empty_input() {
        assert!(
            parse_binary_paths("", Console::default())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn parse_binary_paths_no_binary_files() {
        let numstat = "5\t3\tfile1.txt\x0010\t2\tfile2.rs\0";
        assert!(
            parse_binary_paths(numstat, Console::default())
                .unwrap()
                .is_empty()
        );
    }

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

        async fn commit_files_raw(&self, files: &[(&str, &[u8])], message: &str) -> CommitHash {
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
    async fn commit_files_returns_added_file() {
        let console = Console::default();

        let repo = Repo::new().await;
        let hash = repo
            .commit_files_raw(&[("hello.txt", b"hello")], "add hello")
            .await;
        let result = Extractor::new(repo.path.clone(), console)
            .commit_files(hash.as_ref())
            .await
            .unwrap();

        assert_eq!(
            result.files,
            vec![FileEntry {
                status: FileStatus::Added,
                path: "hello.txt".into(),
                source_path: None,
                is_binary: false,
            }]
        );
    }

    #[tokio::test]
    async fn commit_files_preserves_non_ascii_and_tab_paths() {
        let console = Console::default();
        let repo = Repo::new().await;
        let path = "café\tfile.txt";
        let hash = repo
            .commit_files_raw(&[(path, b"hello")], "add special path")
            .await;

        let result = Extractor::new(repo.path.clone(), console)
            .commit_files(hash.as_ref())
            .await
            .unwrap();

        assert_eq!(
            result.files,
            vec![FileEntry {
                status: FileStatus::Added,
                path: path.into(),
                source_path: None,
                is_binary: false,
            }]
        );
    }

    #[tokio::test]
    async fn commit_files_detects_binary_file() {
        let console = Console::default();

        let repo = Repo::new().await;
        let binary_data: Vec<u8> = vec![0x00, 0x01, 0x02, 0x03];
        let hash = repo
            .commit_files_raw(&[("data.bin", &binary_data)], "add binary")
            .await;
        let result = Extractor::new(repo.path.clone(), console)
            .commit_files(hash.as_ref())
            .await
            .unwrap();

        assert_eq!(
            result.files,
            vec![FileEntry {
                status: FileStatus::Added,
                path: "data.bin".into(),
                source_path: None,
                is_binary: true,
            }]
        );
    }

    #[tokio::test]
    async fn commit_files_detects_modified_and_deleted() {
        let repo = Repo::new().await;
        repo.commit_files_raw(&[("a.txt", b"aaa"), ("b.txt", b"bbb")], "initial")
            .await;

        std::fs::write(repo.path.join("a.txt"), b"changed").unwrap();
        std::fs::remove_file(repo.path.join("b.txt")).unwrap();
        let console = Console::default();
        run_git(&["add", "-A"], &repo.path, console).await.unwrap();
        run_git(
            &["commit", "--no-gpg-sign", "-m", "modify and delete"],
            &repo.path,
            console,
        )
        .await
        .unwrap();
        let raw = run_git(&["rev-parse", "HEAD"], &repo.path, console)
            .await
            .unwrap();
        let hash = CommitHash::new(raw.trim()).unwrap();

        let result = Extractor::new(repo.path.clone(), console)
            .commit_files(hash.as_ref())
            .await
            .unwrap();

        assert_eq!(
            result.files,
            vec![
                FileEntry {
                    status: FileStatus::Modified,
                    path: "a.txt".into(),
                    source_path: None,
                    is_binary: false,
                },
                FileEntry {
                    status: FileStatus::Deleted,
                    path: "b.txt".into(),
                    source_path: None,
                    is_binary: false,
                }
            ]
        );
    }

    #[tokio::test]
    async fn commit_files_tracks_rename_source_path() {
        let console = Console::default();

        let repo = Repo::new().await;
        repo.commit_files_raw(&[("old_name.txt", b"content here")], "initial")
            .await;

        std::fs::rename(
            repo.path.join("old_name.txt"),
            repo.path.join("new_name.txt"),
        )
        .unwrap();
        run_git(&["add", "-A"], &repo.path, console).await.unwrap();
        run_git(
            &["commit", "--no-gpg-sign", "-m", "rename file"],
            &repo.path,
            console,
        )
        .await
        .unwrap();
        let raw = run_git(&["rev-parse", "HEAD"], &repo.path, console)
            .await
            .unwrap();
        let hash = CommitHash::new(raw.trim()).unwrap();

        let result = Extractor::new(repo.path.clone(), console)
            .commit_files(hash.as_ref())
            .await
            .unwrap();

        assert_eq!(
            result.files,
            vec![FileEntry {
                status: FileStatus::Renamed,
                path: "new_name.txt".into(),
                source_path: Some("old_name.txt".into()),
                is_binary: false,
            }]
        );
    }

    #[tokio::test]
    async fn commit_files_fails_for_unknown_hash() {
        let console = Console::default();

        let tmp = tempfile::tempdir().unwrap();
        run_git(&["init"], tmp.path(), console).await.unwrap();
        let hash = "deadbeef";
        let err = Extractor::new(tmp.path().to_path_buf(), console)
            .commit_files(hash)
            .await
            .unwrap_err();

        assert_matches!(err, ExtractError::Git { .. });
    }
}
