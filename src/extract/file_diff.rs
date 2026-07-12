use std::path::{Component, Path};

use serde::{Deserialize, Serialize};

use super::{ExtractError, Extractor};
use crate::git::{CommitHash, run_git};

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
        validate_file_diff_path(path)?;
        let from = CommitHash::resolve(from_revision, &self.project_root, self.console).await?;
        let to = CommitHash::resolve(to_revision, &self.project_root, self.console).await?;
        let path = path.to_string_lossy().into_owned();
        let diff = run_git(
            &[
                "diff",
                "--no-color",
                from.as_ref(),
                to.as_ref(),
                "--",
                &path,
            ],
            &self.project_root,
            self.console,
        )
        .await?;

        Ok(truncate_file_diff(diff))
    }
}

fn validate_file_diff_path(path: &Path) -> Result<(), ExtractError> {
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(ExtractError::InvalidRepositoryRelativePath(
            path.to_path_buf(),
        ));
    }

    Ok(())
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
    use crate::console::Console;
    use crate::git::run_git;

    #[test]
    fn rejects_absolute_and_parent_file_diff_paths() {
        assert!(matches!(
            validate_file_diff_path(Path::new("/tmp/file.rs")),
            Err(ExtractError::InvalidRepositoryRelativePath(_))
        ));
        assert!(matches!(
            validate_file_diff_path(Path::new("src/../file.rs")),
            Err(ExtractError::InvalidRepositoryRelativePath(_))
        ));
    }

    #[test]
    fn truncates_file_diff_at_a_character_boundary() {
        let diff = "한".repeat(MAX_FILE_DIFF_BYTES);
        let result = truncate_file_diff(diff);

        assert!(result.truncated);
        assert!(result.diff.is_char_boundary(result.diff.len()));
        assert!(result.diff.len() <= MAX_FILE_DIFF_BYTES);
    }

    #[tokio::test]
    async fn file_diff_compares_the_requested_revisions_for_one_path() {
        let repository = tempfile::tempdir().unwrap();
        let console = Console::default();
        run_git(&["init"], repository.path(), console)
            .await
            .unwrap();
        run_git(
            &["config", "user.email", "test@example.com"],
            repository.path(),
            console,
        )
        .await
        .unwrap();
        run_git(&["config", "user.name", "Test"], repository.path(), console)
            .await
            .unwrap();
        std::fs::write(repository.path().join("file.txt"), "old\n").unwrap();
        std::fs::write(repository.path().join("other.txt"), "unchanged\n").unwrap();
        run_git(&["add", "."], repository.path(), console)
            .await
            .unwrap();
        run_git(
            &["commit", "--no-gpg-sign", "-m", "initial"],
            repository.path(),
            console,
        )
        .await
        .unwrap();
        std::fs::write(repository.path().join("file.txt"), "new\n").unwrap();
        run_git(&["add", "file.txt"], repository.path(), console)
            .await
            .unwrap();
        run_git(
            &["commit", "--no-gpg-sign", "-m", "update file"],
            repository.path(),
            console,
        )
        .await
        .unwrap();

        let result = Extractor::new(repository.path().to_path_buf(), console)
            .file_diff("HEAD~1", "HEAD", Path::new("file.txt"))
            .await
            .unwrap();

        assert!(result.diff.contains("-old"));
        assert!(result.diff.contains("+new"));
        assert!(!result.diff.contains("other.txt"));
        assert!(!result.truncated);
    }
}
