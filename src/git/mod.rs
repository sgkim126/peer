mod commit_hash;
mod error;

use std::path::Path;

use log::{debug, trace};
use tokio::process::Command;

pub use self::commit_hash::CommitHash;
pub use self::error::GitError;
use self::error::InvalidCommitHashReason;

pub async fn run_git_bytes(args: &[&str], current_dir: &Path) -> Result<Vec<u8>, GitError> {
    let commands = format_argv(args);
    trace!("{commands}");

    let output = Command::new("git")
        .args(args)
        .current_dir(current_dir)
        .kill_on_drop(true)
        .output()
        .await?;

    if !output.status.success() {
        let status = output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        debug!("git command failed command={commands} status={status} stderr={stderr:?}");
        return Err(GitError::NonZeroExit { status, stderr });
    }

    Ok(output.stdout)
}

pub async fn run_git(args: &[&str], current_dir: &Path) -> Result<String, GitError> {
    let stdout = run_git_bytes(args, current_dir).await?;

    Ok(String::from_utf8(stdout)?)
}

fn format_argv(args: &[&str]) -> String {
    format!("git {args:?}")
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::assert_matches;

    #[test]
    fn format_argv_uses_debug_format() {
        assert_eq!(
            format_argv(&["log", "--oneline", "HEAD"]),
            r#"git ["log", "--oneline", "HEAD"]"#
        );
    }

    #[test]
    fn format_argv_with_no_args() {
        assert_eq!(format_argv(&[]), "git []");
    }

    #[tokio::test]
    async fn run_git_succeeds_in_fresh_repo() {
        let tmp = tempfile::tempdir().unwrap();
        run_git(&["init"], tmp.path()).await.unwrap();
    }

    #[tokio::test]
    async fn run_git_returns_non_zero_exit_on_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let err = run_git(&["log"], tmp.path()).await.unwrap_err();
        assert_matches!(err, GitError::NonZeroExit { .. });
    }

    #[tokio::test]
    async fn non_zero_exit_status_is_nonzero() {
        let tmp = tempfile::tempdir().unwrap();
        let err = run_git(&["log"], tmp.path()).await.unwrap_err();
        assert_matches!(err, GitError::NonZeroExit { status, .. } if status != 0);
    }
}
