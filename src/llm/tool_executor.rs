use std::fmt;
use std::path::Path;

use serde::Deserialize;

use crate::extract::{ExtractData, ExtractError, Extractor, FileContent};
use crate::llm::agent::{ToolExecutionResult, ToolExecutor};
use crate::llm::provider::ToolCall;

pub struct PeerToolExecutor {
    extractor: Extractor,
}

impl PeerToolExecutor {
    pub fn new(extractor: Extractor) -> Self {
        Self { extractor }
    }

    async fn execute_tool(&self, call: ToolCall) -> Result<ExtractData, ToolExecutionError> {
        match call.name.as_str() {
            "get_commit_message" => {
                let arguments: RevisionArguments = parse_arguments(&call)?;
                let result = self.extractor.commit_message(&arguments.revision).await?;
                Ok(ExtractData::CommitMessage(result))
            }
            "get_commit_diff" => {
                let arguments: RevisionArguments = parse_arguments(&call)?;
                let result = self.extractor.commit_diff(&arguments.revision).await?;
                Ok(ExtractData::CommitDiff(result))
            }
            "get_changed_files" => {
                let arguments: RevisionArguments = parse_arguments(&call)?;
                let result = self.extractor.commit_files(&arguments.revision).await?;
                Ok(ExtractData::CommitFiles(result))
            }
            "get_commits_in_range" => {
                let arguments: RangeArguments = parse_arguments(&call)?;
                let result = self.extractor.commit_list(&arguments.range).await?;
                Ok(ExtractData::CommitList(result))
            }
            "get_file_content" => {
                let arguments: FileContentArguments = parse_arguments(&call)?;
                let result = self
                    .extractor
                    .file_content(Path::new(&arguments.path), &arguments.revision)
                    .await?;
                Ok(ExtractData::FileContent(result))
            }
            "grep_search" => {
                let arguments: GrepSearchArguments = parse_arguments(&call)?;
                let result = self
                    .extractor
                    .grep_search(
                        &arguments.query,
                        &arguments.revision,
                        arguments.path.as_deref().map(Path::new),
                        arguments.context_lines.unwrap_or_default(),
                    )
                    .await?;
                Ok(ExtractData::GrepSearch(result))
            }
            _ => Err(ToolExecutionError::UnknownTool {
                name: call.name.clone(),
            }),
        }
    }
}

impl ToolExecutor for PeerToolExecutor {
    async fn execute(&self, call: ToolCall) -> ToolExecutionResult {
        self.execute_tool(call)
            .await
            .and_then(tool_result_json)
            .map_err(|error| Box::new(error) as Box<dyn std::error::Error>)
    }
}

#[derive(Deserialize)]
struct RevisionArguments {
    revision: String,
}

#[derive(Deserialize)]
struct RangeArguments {
    range: String,
}

#[derive(Deserialize)]
struct FileContentArguments {
    path: String,
    revision: String,
}

#[derive(Deserialize)]
struct GrepSearchArguments {
    query: String,
    revision: String,
    path: Option<String>,
    context_lines: Option<u8>,
}

fn parse_arguments<T>(call: &ToolCall) -> Result<T, ToolExecutionError>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_value(call.arguments.clone()).map_err(|source| {
        ToolExecutionError::InvalidArguments {
            tool: call.name.clone(),
            source,
        }
    })
}

fn tool_result_json(data: ExtractData) -> Result<serde_json::Value, ToolExecutionError> {
    match data {
        ExtractData::CommitMessage(result) => Ok(serde_json::Value::String(result.message)),
        ExtractData::CommitDiff(result) => Ok(serde_json::Value::String(result.diff)),
        ExtractData::CommitFiles(result) => Ok(serde_json::to_value(result.files)?),
        ExtractData::CommitList(result) => Ok(serde_json::to_value(result.commits)?),
        ExtractData::FileContent(FileContent::Text { content, .. }) => {
            Ok(serde_json::json!({ "type": "text", "content": content }))
        }
        ExtractData::FileContent(FileContent::Binary { size, .. }) => {
            Ok(serde_json::json!({ "type": "binary", "size": size }))
        }
        ExtractData::GrepSearch(result) => Ok(serde_json::to_value(result)?),
    }
}

#[derive(Debug)]
enum ToolExecutionError {
    UnknownTool {
        name: String,
    },
    InvalidArguments {
        tool: String,
        source: serde_json::Error,
    },
    Extract(ExtractError),
    Serialization(serde_json::Error),
}

impl fmt::Display for ToolExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownTool { name } => write!(f, "unknown tool: {name}"),
            Self::InvalidArguments { tool, source } => {
                write!(f, "invalid arguments for {tool}: {source}")
            }
            Self::Extract(error) => error.fmt(f),
            Self::Serialization(error) => write!(f, "cannot serialize tool result: {error}"),
        }
    }
}

impl std::error::Error for ToolExecutionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidArguments { source, .. } => Some(source),
            Self::Extract(error) => Some(error),
            Self::Serialization(error) => Some(error),
            Self::UnknownTool { .. } => None,
        }
    }
}

impl From<ExtractError> for ToolExecutionError {
    fn from(error: ExtractError) -> Self {
        Self::Extract(error)
    }
}

impl From<serde_json::Error> for ToolExecutionError {
    fn from(err: serde_json::Error) -> Self {
        Self::Serialization(err)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::console::Console;
    use crate::git::run_git;

    fn call(name: &str, arguments: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "call-1".to_string(),
            name: name.to_string(),
            arguments,
            thought_signature: None,
        }
    }

    #[tokio::test]
    async fn unknown_tool_returns_model_visible_error() {
        let executor =
            PeerToolExecutor::new(Extractor::new(PathBuf::from("/unused"), Console::default()));

        let error = executor
            .execute(call("unknown", serde_json::json!({})))
            .await
            .unwrap_err();

        assert_eq!(error.to_string(), "unknown tool: unknown");
    }

    #[tokio::test]
    async fn missing_revision_returns_model_visible_error() {
        let executor =
            PeerToolExecutor::new(Extractor::new(PathBuf::from("/unused"), Console::default()));

        let error = executor
            .execute(call("get_commit_diff", serde_json::json!({})))
            .await
            .unwrap_err();

        assert!(
            error
                .to_string()
                .starts_with("invalid arguments for get_commit_diff: missing field `revision`")
        );
    }

    #[tokio::test]
    async fn commit_message_returns_only_the_message() {
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
        std::fs::write(repository.path().join("file.txt"), "content").unwrap();
        run_git(&["add", "file.txt"], repository.path(), console)
            .await
            .unwrap();
        run_git(
            &["commit", "--no-gpg-sign", "-m", "test message"],
            repository.path(),
            console,
        )
        .await
        .unwrap();
        let executor =
            PeerToolExecutor::new(Extractor::new(repository.path().to_path_buf(), console));

        let result = executor
            .execute(call(
                "get_commit_message",
                serde_json::json!({ "revision": "HEAD" }),
            ))
            .await
            .unwrap();

        assert_eq!(result, serde_json::json!("test message"));
    }

    #[tokio::test]
    async fn grep_search_uses_the_requested_commit_snapshot() {
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
        let executor =
            PeerToolExecutor::new(Extractor::new(repository.path().to_path_buf(), console));

        let result = executor
            .execute(call(
                "grep_search",
                serde_json::json!({
                    "query": "validate_token",
                    "revision": "HEAD",
                    "path": "src",
                    "context_lines": 1
                }),
            ))
            .await
            .unwrap();

        assert_eq!(result["truncated"], false);
        assert!(
            result["lines"]
                .as_array()
                .unwrap()
                .iter()
                .any(|line| line.as_str().unwrap().contains("validate_token"))
        );
    }

    #[tokio::test]
    async fn grep_search_returns_an_empty_result_when_there_is_no_match() {
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
        let executor =
            PeerToolExecutor::new(Extractor::new(repository.path().to_path_buf(), console));

        let result = executor
            .execute(call(
                "grep_search",
                serde_json::json!({ "query": "missing", "revision": "HEAD" }),
            ))
            .await
            .unwrap();

        assert_eq!(
            result,
            serde_json::json!({ "lines": [], "truncated": false })
        );
    }

    #[tokio::test]
    async fn grep_search_rejects_invalid_arguments() {
        let executor =
            PeerToolExecutor::new(Extractor::new(PathBuf::from("/unused"), Console::default()));

        let error = executor
            .execute(call(
                "grep_search",
                serde_json::json!({
                    "query": "",
                    "revision": "HEAD",
                    "context_lines": 6
                }),
            ))
            .await
            .unwrap_err();

        assert_eq!(
            error.to_string(),
            "invalid arguments for grep_search: query must not be empty"
        );
    }

    #[test]
    fn file_content_result_omits_internal_metadata() {
        let content = FileContent::Text {
            path: "src/main.rs".to_string(),
            hash: crate::git::CommitHash::new("abc1234").unwrap(),
            content: "fn main() {}".to_string(),
        };

        assert_eq!(
            tool_result_json(ExtractData::FileContent(content)).unwrap(),
            serde_json::json!({
                "type": "text",
                "content": "fn main() {}"
            })
        );
    }
}
