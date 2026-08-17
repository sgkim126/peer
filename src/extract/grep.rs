use std::num::NonZeroU8;
use std::path::Path;

use log::trace;
use serde::{Deserialize, Serialize};

use crate::git::{CommitHash, GitError, run_git};

use super::{ExtractError, Extractor, validate_repository_relative_path};

const MAX_GREP_CONTEXT_LINES: u8 = 10;
const MAX_GREP_RESULT_LINES: usize = 100;

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct GrepResult {
    pub lines: Vec<String>,
    pub truncated: bool,
}

impl Extractor {
    pub async fn grep(
        &self,
        query: &str,
        revision: &str,
        path: Option<&Path>,
        context_lines: NonZeroU8,
    ) -> Result<GrepResult, ExtractError> {
        trace!(
            "extract grep: {revision} query={query:?} path={path:?} context_lines={context_lines}"
        );
        validate_grep_arguments(query, context_lines)?;
        if let Some(path) = path {
            validate_repository_relative_path(path)?;
        }
        let context_lines = context_lines.get().to_string();
        let hash = CommitHash::resolve(revision, &self.project_root, self.console).await?;
        let path = path.map(|path| {
            path.to_str()
                .expect("repository-relative path was validated as UTF-8")
        });
        let mut args: Vec<&str> = vec![
            "--literal-pathspecs",
            "grep",
            "--no-color",
            "-n",
            "-C",
            context_lines.as_str(),
            "-e",
            query,
            hash.as_ref(),
        ];

        if let Some(path) = path {
            args.push("--");
            args.push(path);
        }

        let output = run_git(&args, &self.project_root, self.console)
            .await
            .or_else(|error| match error {
                GitError::NonZeroExit { status: 1, .. } => Ok(String::new()),
                error => Err(error),
            })?;

        Ok(parse_grep_output(&output))
    }
}

fn validate_grep_arguments(query: &str, context_lines: NonZeroU8) -> Result<(), ExtractError> {
    if query.is_empty() {
        return Err(ExtractError::InvalidGrepArguments(
            "query must not be empty".to_string(),
        ));
    }
    if context_lines.get() > MAX_GREP_CONTEXT_LINES {
        return Err(ExtractError::InvalidGrepArguments(format!(
            "context_lines must be at most {MAX_GREP_CONTEXT_LINES}"
        )));
    }

    Ok(())
}

fn parse_grep_output(output: &str) -> GrepResult {
    if output.is_empty() {
        return GrepResult::default();
    }

    let mut lines = output.lines();
    let result_lines = lines
        .by_ref()
        .take(MAX_GREP_RESULT_LINES)
        .map(ToOwned::to_owned)
        .collect();

    GrepResult {
        lines: result_lines,
        truncated: lines.next().is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::assert_matches;

    use crate::console::Console;

    #[test]
    fn rejects_empty_query() {
        assert_matches!(
            validate_grep_arguments("", NonZeroU8::new(1).unwrap()),
            Err(ExtractError::InvalidGrepArguments(_))
        );
    }

    #[test]
    fn rejects_too_many_lines() {
        assert_matches!(
            validate_grep_arguments("query", NonZeroU8::new(11).unwrap()),
            Err(ExtractError::InvalidGrepArguments(_))
        );
    }

    #[test]
    fn rejects_parent_path() {
        assert_matches!(
            validate_repository_relative_path(Path::new("../secret")),
            Err(ExtractError::InvalidRepositoryRelativePath(_))
        );
    }

    #[test]
    fn rejects_empty_path() {
        assert_matches!(
            validate_repository_relative_path(Path::new("")),
            Err(ExtractError::InvalidRepositoryRelativePath(_))
        );
    }

    #[test]
    fn truncates_large_results() {
        let output = (0..=MAX_GREP_RESULT_LINES)
            .map(|index| format!("file.rs:{index}:match"))
            .collect::<Vec<_>>()
            .join("\n");

        let result = parse_grep_output(&output);

        assert_eq!(result.lines.len(), MAX_GREP_RESULT_LINES);
        assert!(result.truncated);
    }

    #[tokio::test]
    async fn searches_the_requested_commit_snapshot() {
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
        std::fs::create_dir_all(repository.path().join("src")).unwrap();
        std::fs::write(
            repository.path().join("src/auth.rs"),
            "fn authenticate() {\n    validate_token();\n}\n",
        )
        .unwrap();
        run_git(&["add", "src/auth.rs"], repository.path(), console)
            .await
            .unwrap();
        run_git(
            &["commit", "--no-gpg-sign", "-m", "add authentication"],
            repository.path(),
            console,
        )
        .await
        .unwrap();
        std::fs::write(
            repository.path().join("src/auth.rs"),
            "fn authenticate() {}\n",
        )
        .unwrap();

        let result = Extractor::new(repository.path().to_path_buf(), console)
            .grep(
                "validate_token",
                "HEAD",
                Some(Path::new("src")),
                NonZeroU8::new(1).unwrap(),
            )
            .await
            .unwrap();

        assert!(!result.truncated);
        assert!(
            result
                .lines
                .iter()
                .any(|line| line.contains("validate_token"))
        );
    }

    #[tokio::test]
    async fn no_match_returns_an_empty_result() {
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
        std::fs::write(repository.path().join("file.txt"), "content\n").unwrap();
        run_git(&["add", "file.txt"], repository.path(), console)
            .await
            .unwrap();
        run_git(
            &["commit", "--no-gpg-sign", "-m", "add file"],
            repository.path(),
            console,
        )
        .await
        .unwrap();

        let result = Extractor::new(repository.path().to_path_buf(), console)
            .grep("missing", "HEAD", None, NonZeroU8::new(2).unwrap())
            .await
            .unwrap();

        assert_eq!(result, GrepResult::default());
    }

    #[tokio::test]
    async fn treats_the_requested_path_as_a_literal_pathspec() {
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
        std::fs::write(repository.path().join(":(glob)**"), "literal match\n").unwrap();
        std::fs::write(repository.path().join("other.txt"), "other match\n").unwrap();
        run_git(&["add", "-A"], repository.path(), console)
            .await
            .unwrap();
        run_git(
            &["commit", "--no-gpg-sign", "-m", "add files"],
            repository.path(),
            console,
        )
        .await
        .unwrap();

        let result = Extractor::new(repository.path().to_path_buf(), console)
            .grep(
                "match",
                "HEAD",
                Some(Path::new(":(glob)**")),
                NonZeroU8::new(1).unwrap(),
            )
            .await
            .unwrap();

        assert!(
            result
                .lines
                .iter()
                .any(|line| line.contains("literal match"))
        );
        assert!(!result.lines.iter().any(|line| line.contains("other match")));
    }
}
