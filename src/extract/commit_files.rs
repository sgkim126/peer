use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::ExtractError;
use crate::console::Console;
use crate::git::{CommitHash, run_git};

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

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub status: FileStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    pub is_binary: bool,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
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

fn parse_name_status(output: &str, console: Console) -> Vec<RawFileEntry> {
    let mut entries = Vec::new();
    for line in output.lines() {
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.splitn(3, '\t').collect();
        if parts.is_empty() {
            continue;
        }
        // git appends a similarity score to R and C codes (e.g. R90, C80)
        let status = match parts[0].chars().next() {
            Some('R') => FileStatus::Renamed,
            Some('C') => FileStatus::Copied,
            Some('A') => FileStatus::Added,
            Some('M') => FileStatus::Modified,
            Some('D') => FileStatus::Deleted,
            Some('T') => FileStatus::TypeChanged,
            _ => {
                console.debug(format!("skipping unknown git name-status line: {line}"));
                continue;
            }
        };
        let entry = match status {
            FileStatus::Renamed | FileStatus::Copied => {
                let path = parts.get(2).copied().unwrap_or("").to_string();
                let source_path = parts.get(1).map(|s| s.to_string());
                RawFileEntry {
                    status,
                    path,
                    source_path,
                }
            }
            _ => {
                let path = parts.get(1).copied().unwrap_or("").to_string();
                RawFileEntry {
                    status,
                    path,
                    source_path: None,
                }
            }
        };
        entries.push(entry);
    }
    entries
}

fn parse_binary_paths(numstat: &str) -> HashSet<&str> {
    let mut binary = HashSet::new();
    for line in numstat.lines() {
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(3, '\t');
        let added = parts.next().unwrap_or("");
        let deleted = parts.next().unwrap_or("");
        let path = parts.next().unwrap_or("");
        if added == "-" && deleted == "-" {
            binary.insert(path);
        }
    }
    binary
}

pub async fn commit_files(
    hash: CommitHash,
    project_root: &Path,
    console: Console,
) -> Result<CommitFiles, ExtractError> {
    let hash_str: &str = hash.as_ref();

    let name_status_out = run_git(
        &[
            "diff-tree",
            "--no-commit-id",
            "--root",
            "-r",
            "--name-status",
            "-M",
            "-C",
            hash_str,
        ],
        project_root,
        console,
    )
    .await?;

    let numstat_out = run_git(
        &[
            "diff-tree",
            "--no-commit-id",
            "--root",
            "-r",
            "--numstat",
            hash_str,
        ],
        project_root,
        console,
    )
    .await?;

    let binary_paths = parse_binary_paths(&numstat_out);
    let raw_entries = parse_name_status(&name_status_out, console);

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

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    use crate::console::Console;

    #[test]
    fn parse_name_status_added() {
        let console = Console::default();

        let entries = parse_name_status("A\tsrc/new_file.rs\n", console);
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

        let entries = parse_name_status("M\tsrc/lib.rs\n", console);
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

        let entries = parse_name_status("D\tsrc/old_file.rs\n", console);
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

        let entries = parse_name_status("R90\tsrc/old_name.rs\tsrc/new_name.rs\n", console);
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

        let entries = parse_name_status("C80\tsrc/original.rs\tsrc/copy.rs\n", console);
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

        let entries = parse_name_status("T\tsrc/some_file.rs\n", console);
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

        let output = "A\tfile1.txt\nM\tfile2.rs\nD\tfile3.old\n";
        let entries = parse_name_status(output, console);
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
    fn parse_name_status_skips_unknown_codes() {
        let console = Console::default();

        let output = "X\tunknown.txt\nA\tknown.txt\n";
        let entries = parse_name_status(output, console);
        assert_eq!(
            entries,
            vec![RawFileEntry {
                status: FileStatus::Added,
                path: "known.txt".into(),
                source_path: None,
            }]
        );
    }

    #[test]
    fn parse_name_status_empty_input() {
        let console = Console::default();

        assert_eq!(parse_name_status("", console), Vec::<RawFileEntry>::new());
    }

    #[test]
    fn parse_binary_paths_detects_binary() {
        let binary1 = "image.png";
        let non_binary1 = "file.txt";
        let binary2 = "archive.zip";
        let numstat = format!("-\t-\t{binary1}\n5\t3\t{non_binary1}\n-\t-\t{binary2}\n");
        let binary = parse_binary_paths(&numstat);
        assert!(binary.contains(binary1));
        assert!(binary.contains(binary2));
        assert!(!binary.contains(non_binary1));
    }

    #[test]
    fn parse_binary_paths_empty_input() {
        assert!(parse_binary_paths("").is_empty());
    }

    #[test]
    fn parse_binary_paths_no_binary_files() {
        let numstat = "5\t3\tfile1.txt\n10\t2\tfile2.rs\n";
        assert!(parse_binary_paths(numstat).is_empty());
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
        let result = commit_files(hash, &repo.path, console).await.unwrap();

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
    async fn commit_files_detects_binary_file() {
        let console = Console::default();

        let repo = Repo::new().await;
        let binary_data: Vec<u8> = vec![0x00, 0x01, 0x02, 0x03];
        let hash = repo
            .commit_files_raw(&[("data.bin", &binary_data)], "add binary")
            .await;
        let result = commit_files(hash, &repo.path, console).await.unwrap();

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

        let result = commit_files(hash, &repo.path, console).await.unwrap();

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

        let result = commit_files(hash, &repo.path, console).await.unwrap();

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
        let hash = CommitHash::new("deadbeef").unwrap();
        let err = commit_files(hash, tmp.path(), console).await.unwrap_err();

        assert!(matches!(err, ExtractError::Git { .. }));
    }
}
