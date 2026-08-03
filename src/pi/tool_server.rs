use std::fmt;
use std::num::NonZeroU8;
use std::path::Path;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::extract::{ExtractError, Extractor, FileContent};

#[derive(Debug)]
enum ToolExecutionError {
    UnknownTool(String),
    InvalidArguments { tool: String, reason: String },
    Extract(ExtractError),
    Serialization(serde_json::Error),
}

impl fmt::Display for ToolExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownTool(tool) => write!(f, "unknown peer read tool: {tool}"),
            Self::InvalidArguments { tool, reason } => {
                write!(f, "invalid arguments for {tool}: {reason}")
            }
            Self::Extract(error) => error.fmt(f),
            Self::Serialization(error) => write!(f, "cannot serialize tool result: {error}"),
        }
    }
}

impl std::error::Error for ToolExecutionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::UnknownTool(_) => None,
            Self::InvalidArguments { .. } => None,
            Self::Extract(error) => Some(error),
            Self::Serialization(error) => Some(error),
        }
    }
}

impl From<ExtractError> for ToolExecutionError {
    fn from(error: ExtractError) -> Self {
        Self::Extract(error)
    }
}

impl From<serde_json::Error> for ToolExecutionError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialization(error)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RevisionArguments {
    revision: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RangeArguments {
    from_revision: String,
    to_revision: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileArguments {
    revision: String,
    path: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileDiffArguments {
    from_revision: String,
    to_revision: String,
    path: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListTreeArguments {
    revision: String,
    path: Option<String>,
    #[serde(default)]
    recursive: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GrepArguments {
    revision: String,
    query: String,
    path: Option<String>,
    context_lines: Option<NonZeroU8>,
}

#[cfg_attr(not(test), expect(dead_code))]
async fn execute_tool(
    extractor: &Extractor,
    tool: &str,
    arguments: Value,
) -> Result<Value, ToolExecutionError> {
    match tool {
        "get_commit_message" => {
            let arguments: RevisionArguments = parse_arguments(tool, arguments)?;
            let result = extractor.commit_message(&arguments.revision).await?;
            Ok(Value::String(result.message))
        }
        "get_commit_diff" => {
            let arguments: RevisionArguments = parse_arguments(tool, arguments)?;
            let result = extractor.commit_diff(&arguments.revision).await?;
            Ok(Value::String(result.diff))
        }
        "get_changed_files" => {
            let arguments: RevisionArguments = parse_arguments(tool, arguments)?;
            let result = extractor.commit_files(&arguments.revision).await?;
            Ok(serde_json::to_value(result.files)?)
        }
        "get_commits_in_range" => {
            let arguments: RangeArguments = parse_arguments(tool, arguments)?;
            let range = format!("{}..{}", arguments.from_revision, arguments.to_revision);
            let result = extractor.commit_list(&range).await?;
            Ok(serde_json::to_value(result.commits)?)
        }
        "get_file_content" => {
            let arguments: FileArguments = parse_arguments(tool, arguments)?;
            let result = extractor
                .file_content(&arguments.revision, Path::new(&arguments.path), None)
                .await?;
            match result {
                FileContent::Text { content, .. } => Ok(json!({
                    "type": "text",
                    "content": content
                })),
                FileContent::Binary { size, .. } => Ok(json!({
                    "type": "binary",
                    "size": size
                })),
            }
        }
        "get_file_diff" => {
            let arguments: FileDiffArguments = parse_arguments(tool, arguments)?;
            let result = extractor
                .file_diff(
                    &arguments.from_revision,
                    &arguments.to_revision,
                    Path::new(&arguments.path),
                )
                .await?;
            Ok(serde_json::to_value(result)?)
        }
        "list_tree" => {
            let arguments: ListTreeArguments = parse_arguments(tool, arguments)?;
            let result = extractor
                .list_tree(
                    &arguments.revision,
                    arguments.path.as_deref().map(Path::new),
                    arguments.recursive,
                )
                .await?;
            Ok(serde_json::to_value(result)?)
        }
        "grep" => {
            let arguments: GrepArguments = parse_arguments(tool, arguments)?;
            let context_lines = arguments
                .context_lines
                .unwrap_or(NonZeroU8::new(2).expect("2 is non-zero"));
            let result = extractor
                .grep(
                    &arguments.query,
                    &arguments.revision,
                    arguments.path.as_deref().map(Path::new),
                    context_lines,
                )
                .await?;
            Ok(serde_json::to_value(result)?)
        }
        _ => Err(ToolExecutionError::UnknownTool(tool.to_string())),
    }
}

fn parse_arguments<T>(tool: &str, arguments: Value) -> Result<T, ToolExecutionError>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_value(arguments).map_err(|error| ToolExecutionError::InvalidArguments {
        tool: tool.to_string(),
        reason: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use crate::console::Console;

    #[tokio::test]
    async fn reports_unknown_tools() {
        let extractor = Extractor::new(PathBuf::from("/unused"), Console::default());
        let error = execute_tool(&extractor, "unknown", json!({}))
            .await
            .unwrap_err();

        assert_eq!(error.to_string(), "unknown peer read tool: unknown");
    }
}
