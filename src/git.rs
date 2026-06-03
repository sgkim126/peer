use std::fmt;
use std::path::Path;

use tokio::process::Command;

use crate::console::Console;

pub async fn run_git(
    args: &[&str],
    current_dir: &Path,
    console: Console,
) -> Result<String, GitError> {
    let commands = format_argv(args);
    console.debug(&commands);

    let output = Command::new("git")
        .args(args)
        .current_dir(current_dir)
        .kill_on_drop(true)
        .output()
        .await
        .map_err(GitError::Spawn)?;

    if !output.status.success() {
        let status = output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        console.debug(format!("{commands}: ({status}): {stderr}"));
        return Err(GitError::NonZeroExit { status, stderr });
    }

    String::from_utf8(output.stdout).map_err(GitError::FromUtf8)
}

fn format_argv(args: &[&str]) -> String {
    std::iter::once("git")
        .chain(args.iter().copied())
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Debug)]
pub enum GitError {
    Spawn(std::io::Error),
    NonZeroExit { status: i32, stderr: String },
    FromUtf8(std::string::FromUtf8Error),
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GitError::Spawn(e) => write!(f, "failed to spawn git: {e}"),
            GitError::NonZeroExit { status, stderr } => {
                write!(f, "git exited with status {status}: {stderr}")
            }
            GitError::FromUtf8(e) => write!(f, "git output is not valid UTF-8: {e}"),
        }
    }
}

impl std::error::Error for GitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            GitError::Spawn(e) => Some(e),
            GitError::NonZeroExit { .. } => None,
            GitError::FromUtf8(e) => Some(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_argv_joins_with_spaces() {
        assert_eq!(
            format_argv(&["log", "--oneline", "HEAD"]),
            "git log --oneline HEAD"
        );
    }

    #[test]
    fn format_argv_with_no_args() {
        assert_eq!(format_argv(&[]), "git");
    }

    #[tokio::test]
    async fn run_git_succeeds_in_fresh_repo() {
        let tmp = tempfile::tempdir().unwrap();
        run_git(&["init"], tmp.path(), Console::default())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn run_git_returns_non_zero_exit_on_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let err = run_git(&["log"], tmp.path(), Console::default())
            .await
            .unwrap_err();
        assert!(matches!(err, GitError::NonZeroExit { .. }));
    }

    #[tokio::test]
    async fn non_zero_exit_status_is_nonzero() {
        let tmp = tempfile::tempdir().unwrap();
        let err = run_git(&["log"], tmp.path(), Console::default())
            .await
            .unwrap_err();
        assert!(matches!(err, GitError::NonZeroExit { status, .. } if status != 0));
    }
}
