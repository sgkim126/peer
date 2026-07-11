use std::path::{Component, Path};

use serde::{Deserialize, Serialize};

use super::{ExtractError, Extractor};
use crate::git::{CommitHash, GitError, run_git};

const MAX_GREP_CONTEXT_LINES: u8 = 10;
const MAX_GREP_RESULT_LINES: usize = 100;

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct GrepSearchResult {
    pub lines: Vec<String>,
    pub truncated: bool,
}

impl Extractor {
    pub async fn grep_search(
        &self,
        query: &str,
        revision: &str,
        path: Option<&Path>,
        context_lines: u8,
    ) -> Result<GrepSearchResult, ExtractError> {
        validate_grep_search_arguments(query, path, context_lines)?;
        let hash = CommitHash::resolve(revision, &self.project_root, self.console).await?;
        let mut args = vec![
            "grep".to_string(),
            "--no-color".to_string(),
            "-n".to_string(),
            "-C".to_string(),
            context_lines.to_string(),
            "-e".to_string(),
            query.to_string(),
            hash.to_string(),
        ];

        if let Some(path) = path {
            args.push("--".to_string());
            args.push(path.to_string_lossy().into_owned());
        }

        let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();
        let output = run_git(&arg_refs, &self.project_root, self.console)
            .await
            .or_else(|error| match error {
                GitError::NonZeroExit { status: 1, .. } => Ok(String::new()),
                error => Err(error),
            })?;

        if output.is_empty() {
            return Ok(GrepSearchResult::default());
        }

        let mut lines = output.lines();
        let result_lines = lines
            .by_ref()
            .take(MAX_GREP_RESULT_LINES)
            .map(ToOwned::to_owned)
            .collect();

        Ok(GrepSearchResult {
            lines: result_lines,
            truncated: lines.next().is_some(),
        })
    }
}

fn validate_grep_search_arguments(
    query: &str,
    path: Option<&Path>,
    context_lines: u8,
) -> Result<(), ExtractError> {
    if query.is_empty() {
        return Err(ExtractError::InvalidGrepSearchArguments(
            "query must not be empty".to_string(),
        ));
    }

    if let Some(path) = path
        && (path.is_absolute()
            || path
                .components()
                .any(|component| matches!(component, Component::ParentDir)))
    {
        return Err(ExtractError::InvalidGrepSearchArguments(
            "path must be repository-relative".to_string(),
        ));
    }

    if context_lines > MAX_GREP_CONTEXT_LINES {
        return Err(ExtractError::InvalidGrepSearchArguments(format!(
            "context_lines must be at most {MAX_GREP_CONTEXT_LINES}"
        )));
    }

    Ok(())
}
