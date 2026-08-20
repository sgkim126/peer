mod commit_diff;
mod commit_files;
mod commit_list;
mod commit_message;
mod error;
mod file_content;
mod file_diff;
mod grep;
mod list_tree;
mod range_diff;

use std::path::{Component, Path, PathBuf};

pub use self::commit_files::CommitFiles;
pub use self::commit_message::CommitMessage;
pub use self::error::ExtractError;
#[expect(
    unused_imports,
    reason = "retained until Pi's get_file_content tool supports range queries"
)]
pub use self::file_content::{FileContent, FileContentRange};

/// Provides the programmatic entry point to repository extraction.
pub struct Extractor {
    project_root: PathBuf,
}

impl Extractor {
    pub fn new(project_root: PathBuf) -> Self {
        Self { project_root }
    }
}

fn validate_repository_relative_path(path: &Path) -> Result<(), ExtractError> {
    if path.as_os_str().is_empty()
        || path.to_str().is_none()
        || path.is_absolute()
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

#[cfg(test)]
mod tests {
    use super::*;

    use std::assert_matches;

    #[cfg(unix)]
    #[test]
    fn rejects_non_utf8_path() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let path = PathBuf::from(OsString::from_vec(b"invalid-\xff.txt".to_vec()));
        let error = validate_repository_relative_path(&path).unwrap_err();

        assert_matches!(&error, ExtractError::InvalidRepositoryRelativePath(_));
        assert_eq!(
            error.to_string(),
            "repository-relative path must be valid UTF-8"
        );
    }
}
