use std::path::Path;

use log::trace;
use serde::{Deserialize, Serialize};

use crate::git::{CommitHash, run_git};

use super::{ExtractError, Extractor, validate_repository_relative_path};

const MAX_FILE_DIFF_BYTES: usize = 100 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct FileDiff {
    pub diff: String,
    pub truncated: bool,
}

impl Extractor {
    pub async fn file_diff(
        &self,
        from_revision: &str,
        to_revision: &str,
        path: &Path,
    ) -> Result<FileDiff, ExtractError> {
        trace!(
            "extract file diff: {from_revision}..{to_revision} {}",
            path.display()
        );
        validate_repository_relative_path(path)?;
        let from = CommitHash::resolve(from_revision, &self.project_root).await?;
        let to = CommitHash::resolve(to_revision, &self.project_root).await?;
        let path = path
            .to_str()
            .expect("repository-relative path was validated as UTF-8");
        let diff = run_git(
            &[
                "--literal-pathspecs",
                "diff",
                "--no-color",
                from.as_ref(),
                to.as_ref(),
                "--",
                path,
            ],
            &self.project_root,
        )
        .await?;

        Ok(truncate_file_diff(diff))
    }
}

fn truncate_file_diff(diff: String) -> FileDiff {
    if diff.len() <= MAX_FILE_DIFF_BYTES {
        return FileDiff {
            diff,
            truncated: false,
        };
    }

    let mut end = MAX_FILE_DIFF_BYTES;
    while !diff.is_char_boundary(end) {
        end -= 1;
    }

    FileDiff {
        diff: diff[..end].to_string(),
        truncated: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::assert_matches;

    #[test]
    fn rejects_empty_path() {
        assert_matches!(
            validate_repository_relative_path(Path::new("")),
            Err(ExtractError::InvalidRepositoryRelativePath(_))
        );
    }

    #[test]
    fn rejects_absolute_path() {
        assert_matches!(
            validate_repository_relative_path(Path::new("/tmp/file.rs")),
            Err(ExtractError::InvalidRepositoryRelativePath(_))
        );
    }

    #[test]
    fn rejects_parent_path() {
        assert_matches!(
            validate_repository_relative_path(Path::new("src/../file.rs")),
            Err(ExtractError::InvalidRepositoryRelativePath(_))
        );
    }

    #[test]
    fn truncates_at_a_character_boundary() {
        let diff = "a".repeat(MAX_FILE_DIFF_BYTES + 100);
        let result = truncate_file_diff(diff);

        assert!(result.truncated);
        assert!(result.diff.is_char_boundary(result.diff.len()));
        assert!(result.diff.len() <= MAX_FILE_DIFF_BYTES);
    }

    #[tokio::test]
    async fn compares_two_revisions_for_one_path() {
        let repository = tempfile::tempdir().unwrap();
        run_git(&["init"], repository.path()).await.unwrap();
        run_git(
            &["config", "user.email", "test@example.com"],
            repository.path(),
        )
        .await
        .unwrap();
        run_git(&["config", "user.name", "Test"], repository.path())
            .await
            .unwrap();
        std::fs::write(repository.path().join("file.txt"), "old\n").unwrap();
        std::fs::write(repository.path().join("other.txt"), "unchanged\n").unwrap();
        run_git(&["add", "."], repository.path()).await.unwrap();
        run_git(
            &["commit", "--no-gpg-sign", "-m", "initial"],
            repository.path(),
        )
        .await
        .unwrap();
        std::fs::write(repository.path().join("file.txt"), "new\n").unwrap();
        run_git(&["add", "file.txt"], repository.path())
            .await
            .unwrap();
        run_git(
            &["commit", "--no-gpg-sign", "-m", "update file"],
            repository.path(),
        )
        .await
        .unwrap();

        let result = Extractor::new(repository.path().to_path_buf())
            .file_diff("HEAD~1", "HEAD", Path::new("file.txt"))
            .await
            .unwrap();

        assert!(result.diff.contains("-old"));
        assert!(result.diff.contains("+new"));
        assert!(!result.diff.contains("other.txt"));
        assert!(!result.truncated);
    }

    #[tokio::test]
    async fn treats_the_requested_path_as_a_literal_pathspec() {
        let repository = tempfile::tempdir().unwrap();
        run_git(&["init"], repository.path()).await.unwrap();
        run_git(
            &["config", "user.email", "test@example.com"],
            repository.path(),
        )
        .await
        .unwrap();
        run_git(&["config", "user.name", "Test"], repository.path())
            .await
            .unwrap();
        std::fs::write(repository.path().join("*.txt"), "literal old\n").unwrap();
        std::fs::write(repository.path().join("other.txt"), "other old\n").unwrap();
        run_git(&["add", "-A"], repository.path()).await.unwrap();
        run_git(
            &["commit", "--no-gpg-sign", "-m", "initial"],
            repository.path(),
        )
        .await
        .unwrap();
        std::fs::write(repository.path().join("*.txt"), "literal new\n").unwrap();
        std::fs::write(repository.path().join("other.txt"), "other new\n").unwrap();
        run_git(&["add", "-A"], repository.path()).await.unwrap();
        run_git(
            &["commit", "--no-gpg-sign", "-m", "update files"],
            repository.path(),
        )
        .await
        .unwrap();

        let result = Extractor::new(repository.path().to_path_buf())
            .file_diff("HEAD~1", "HEAD", Path::new("*.txt"))
            .await
            .unwrap();

        assert!(result.diff.contains("literal old"));
        assert!(result.diff.contains("literal new"));
        assert!(!result.diff.contains("other old"));
        assert!(!result.diff.contains("other new"));
    }
}
