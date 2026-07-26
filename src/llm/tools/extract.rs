#![cfg_attr(not(test), expect(dead_code))]

use std::num::NonZeroU8;
use std::path::Path;

use serde::{Deserialize, Deserializer, de};
use serde_json::json;

use crate::extract::{ExtractData, Extractor, FileContent, FileContentRange};

use super::super::{ToolCall, ToolSpec};
use super::{ToolExecutionError, ToolExecutionResult, ToolExecutor};

#[cfg_attr(not(test), expect(dead_code))]
fn get_commit_message() -> ToolSpec {
    ToolSpec {
        name: "get_commit_message".to_string(),
        description: "Returns the full commit message for a commit.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "revision": {
                    "type": "string",
                    "description": "Git revision resolving to a commit. Prefer an explicit commit hash to avoid ambiguity."
                }
            },
            "required": ["revision"]
        }),
    }
}

pub fn get_commit_diff() -> ToolSpec {
    ToolSpec {
        name: "get_commit_diff".to_string(),
        description: "Returns the full unified diff for a commit.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "revision": {
                    "type": "string",
                    "description": "Git revision resolving to a commit. Prefer an explicit commit hash to avoid ambiguity."
                }
            },
            "required": ["revision"]
        }),
    }
}

pub fn get_changed_files() -> ToolSpec {
    ToolSpec {
        name: "get_changed_files".to_string(),
        description: "Returns the files changed in a commit.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "revision": {
                    "type": "string",
                    "description": "Git revision resolving to a commit. Prefer an explicit commit hash to avoid ambiguity."
                }
            },
            "required": ["revision"]
        }),
    }
}

#[cfg_attr(not(test), expect(dead_code))]
fn get_commits_in_range() -> ToolSpec {
    ToolSpec {
        name: "get_commits_in_range".to_string(),
        description: "Returns commit hashes in a two-dot range, oldest to newest.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "range": {
                    "type": "string",
                    "description": "Git two-dot range. Prefer explicit commit hashes on both sides to avoid ambiguity."
                }
            },
            "required": ["range"]
        }),
    }
}

pub fn get_file_content() -> ToolSpec {
    ToolSpec {
        name: "get_file_content".to_string(),
        description: "Returns a file's content at a commit.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "revision": {
                    "type": "string",
                    "description": "Git revision at which to read the file. Prefer an explicit commit hash to avoid ambiguity."
                },
                "path": {
                    "type": "string",
                    "description": "Repository-root-relative path."
                },
                "start_line": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Optional first line to return. Provide only together with end_line."
                },
                "end_line": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Optional last line to return. Provide only together with start_line."
                }
            },
            "required": ["revision", "path"]
        }),
    }
}

pub fn get_file_diff() -> ToolSpec {
    ToolSpec {
        name: "get_file_diff".to_string(),
        description: "Returns the diff for one file between two Git revisions.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "from_revision": {
                    "type": "string",
                    "description": "Earlier Git revision. Prefer an explicit commit hash to avoid ambiguity."
                },
                "to_revision": {
                    "type": "string",
                    "description": "Later Git revision. Prefer an explicit commit hash to avoid ambiguity."
                },
                "path": {
                    "type": "string",
                    "description": "Repository-root-relative file path to compare."
                }
            },
            "required": ["from_revision", "to_revision", "path"]
        }),
    }
}

pub fn list_tree() -> ToolSpec {
    ToolSpec {
        name: "list_tree".to_string(),
        description: "Lists files and directories in a repository tree at a commit.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "revision": {
                    "type": "string",
                    "description": "Git revision whose tree to list. Prefer an explicit commit hash to avoid ambiguity."
                },
                "path": {
                    "type": "string",
                    "description": "Repository-root-relative directory to list."
                },
                "recursive": {
                    "type": "boolean",
                    "description": "Whether to include entries below the selected directory."
                }
            },
            "required": ["revision", "path"]
        }),
    }
}

pub fn grep() -> ToolSpec {
    ToolSpec {
        name: "grep".to_string(),
        description: "Searches a commit snapshot for literal text and returns matching lines with optional surrounding context.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Literal text to search for."
                },
                "revision": {
                    "type": "string",
                    "description": "Git revision whose snapshot to search. Prefer an explicit commit hash to avoid ambiguity."
                },
                "path": {
                    "type": "string",
                    "description": "Optional repository-root-relative file or directory to search."
                },
                "context_lines": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 10,
                    "description": "Optional number of surrounding lines to return for each match. Defaults to 2."
                }
            },
            "required": ["query", "revision"]
        }),
    }
}

pub struct ExtractToolExecutor {
    extractor: Extractor,
}

impl ExtractToolExecutor {
    pub fn new(extractor: Extractor) -> Self {
        Self { extractor }
    }
}

impl ToolExecutor for ExtractToolExecutor {
    async fn execute(&self, call: ToolCall) -> ToolExecutionResult {
        let data = match call.name.as_str() {
            "get_commit_message" => {
                let arguments: RevisionArguments = parse_arguments(&call)?;
                let result = self.extractor.commit_message(&arguments.revision).await?;
                ExtractData::CommitMessage(result)
            }
            "get_commit_diff" => {
                let arguments: RevisionArguments = parse_arguments(&call)?;
                let result = self.extractor.commit_diff(&arguments.revision).await?;
                ExtractData::CommitDiff(result)
            }
            "get_changed_files" => {
                let arguments: RevisionArguments = parse_arguments(&call)?;
                let result = self.extractor.commit_files(&arguments.revision).await?;
                ExtractData::CommitFiles(result)
            }
            "get_commits_in_range" => {
                let arguments: RangeArguments = parse_arguments(&call)?;
                let result = self.extractor.commit_list(&arguments.range).await?;
                ExtractData::CommitList(result)
            }
            "get_file_content" => {
                let arguments: FileContentArguments = parse_arguments(&call)?;
                let result = self
                    .extractor
                    .file_content(
                        &arguments.revision,
                        Path::new(&arguments.path),
                        arguments.line_range.0,
                    )
                    .await?;
                ExtractData::FileContent(result)
            }
            "get_file_diff" => {
                let arguments: FileDiffArguments = parse_arguments(&call)?;
                let result = self
                    .extractor
                    .file_diff(
                        &arguments.from_revision,
                        &arguments.to_revision,
                        Path::new(&arguments.path),
                    )
                    .await?;
                ExtractData::FileDiff(result)
            }
            "list_tree" => {
                let arguments: ListTreeArguments = parse_arguments(&call)?;
                let result = self
                    .extractor
                    .list_tree(
                        &arguments.revision,
                        Some(Path::new(&arguments.path)),
                        arguments.recursive,
                    )
                    .await?;
                ExtractData::ListTree(result)
            }
            "grep" => {
                let arguments: GrepArguments = parse_arguments(&call)?;
                let result = self
                    .extractor
                    .grep(
                        &arguments.query,
                        &arguments.revision,
                        arguments.path.as_deref().map(Path::new),
                        arguments
                            .context_lines
                            .unwrap_or(NonZeroU8::new(2).expect("2 is non-zero")),
                    )
                    .await?;
                ExtractData::Grep(result)
            }
            _ => {
                return Err(ToolExecutionError::UnknownTool { name: call.name });
            }
        };
        Ok(data.try_into()?)
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
    revision: String,
    path: String,
    #[serde(flatten)]
    line_range: OptionalFileContentRange,
}

struct OptionalFileContentRange(Option<FileContentRange>);

impl<'de> Deserialize<'de> for OptionalFileContentRange {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawOptionalFileContentRange {
            start_line: Option<u32>,
            end_line: Option<u32>,
        }

        let raw = RawOptionalFileContentRange::deserialize(deserializer)?;
        let range = match (raw.start_line, raw.end_line) {
            (None, None) => None,
            (Some(start_line), Some(end_line)) => {
                Some(FileContentRange::new(start_line, end_line).map_err(de::Error::custom)?)
            }
            _ => {
                return Err(de::Error::custom(
                    "start_line and end_line must be provided together",
                ));
            }
        };
        Ok(Self(range))
    }
}

#[derive(Deserialize)]
struct FileDiffArguments {
    from_revision: String,
    to_revision: String,
    path: String,
}

#[derive(Deserialize)]
struct ListTreeArguments {
    revision: String,
    path: String,
    #[serde(default)]
    recursive: bool,
}

#[derive(Deserialize)]
struct GrepArguments {
    query: String,
    revision: String,
    path: Option<String>,
    context_lines: Option<NonZeroU8>,
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

impl TryFrom<ExtractData> for serde_json::Value {
    type Error = serde_json::Error;

    fn try_from(data: ExtractData) -> Result<Self, Self::Error> {
        match data {
            ExtractData::CommitMessage(result) => Ok(serde_json::Value::String(result.message)),
            ExtractData::CommitDiff(result) => Ok(serde_json::Value::String(result.diff)),
            ExtractData::CommitFiles(result) => Ok(serde_json::to_value(result.files)?),
            ExtractData::CommitList(result) => Ok(serde_json::to_value(result.commits)?),
            ExtractData::FileContent(FileContent::Text { content, range, .. }) => {
                let mut result = json!({ "type": "text", "content": content });
                if let Some(range) = range {
                    result["start_line"] = json!(range.start_line());
                    result["end_line"] = json!(range.end_line());
                }
                Ok(result)
            }
            ExtractData::FileContent(FileContent::Binary { size, .. }) => {
                Ok(json!({ "type": "binary", "size": size }))
            }
            ExtractData::FileDiff(result) => Ok(serde_json::to_value(result)?),
            ExtractData::Grep(result) => Ok(serde_json::to_value(result)?),
            ExtractData::ListTree(result) => Ok(serde_json::to_value(result)?),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use crate::console::Console;
    use crate::git::run_git;

    fn call(name: &str, arguments: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "call-1".to_string(),
            name: name.to_string(),
            arguments,
            provider_state: None,
        }
    }

    fn unused_executor() -> ExtractToolExecutor {
        ExtractToolExecutor::new(Extractor::new(PathBuf::from("/unused"), Console::default()))
    }

    #[test]
    fn schemas_require_the_expected_arguments() {
        assert_eq!(
            get_commit_message().parameters["required"],
            json!(["revision"])
        );
        assert_eq!(
            get_commit_diff().parameters["required"],
            json!(["revision"])
        );
        assert_eq!(
            get_changed_files().parameters["required"],
            json!(["revision"])
        );
        assert_eq!(
            get_commits_in_range().parameters["required"],
            json!(["range"])
        );
        assert_eq!(
            get_file_content().parameters["required"],
            json!(["revision", "path"])
        );
        assert_eq!(
            get_file_content().parameters["properties"]["start_line"]["minimum"],
            1
        );
        assert_eq!(
            get_file_content().parameters["properties"]["end_line"]["minimum"],
            1
        );
        assert_eq!(
            get_file_diff().parameters["required"],
            json!(["from_revision", "to_revision", "path"])
        );
        assert_eq!(
            list_tree().parameters["required"],
            json!(["revision", "path"])
        );
        assert_eq!(grep().parameters["required"], json!(["query", "revision"]));
        assert_eq!(
            grep().parameters["properties"]["context_lines"]["minimum"],
            1
        );
        assert_eq!(
            grep().parameters["properties"]["context_lines"]["maximum"],
            10
        );
    }

    #[test]
    fn file_content_line_range_must_be_complete_and_valid() {
        let valid: FileContentArguments = serde_json::from_value(json!({
            "revision": "HEAD",
            "path": "src/main.rs",
            "start_line": 10,
            "end_line": 20
        }))
        .unwrap();
        assert_eq!(
            valid.line_range.0,
            Some(FileContentRange::new(10, 20).unwrap())
        );

        for arguments in [
            json!({
                "revision": "HEAD",
                "path": "src/main.rs",
                "start_line": 10
            }),
            json!({
                "revision": "HEAD",
                "path": "src/main.rs",
                "start_line": 20,
                "end_line": 10
            }),
        ] {
            assert!(serde_json::from_value::<FileContentArguments>(arguments).is_err());
        }
    }

    #[test]
    fn grep_arguments_of_context_lines_default_to_two() {
        let arguments: GrepArguments = serde_json::from_value(json!({
            "query": "needle",
            "revision": "HEAD"
        }))
        .unwrap();
        assert_eq!(
            arguments
                .context_lines
                .unwrap_or(NonZeroU8::new(2).expect("2 is non-zero"))
                .get(),
            2
        );
    }

    #[test]
    fn grep_rejects_zero_context_lines() {
        let arguments: Result<GrepArguments, _> = serde_json::from_value(json!({
            "query": "needle",
            "revision": "HEAD",
            "context_lines": 0
        }));
        assert!(arguments.is_err());
    }

    #[tokio::test]
    async fn rejects_unknown_tools() {
        let error = unused_executor()
            .execute(call("unknown", json!({})))
            .await
            .unwrap_err();

        assert_eq!(error.to_string(), "unknown tool: unknown");
    }

    #[tokio::test]
    async fn reports_invalid_arguments() {
        let error = unused_executor()
            .execute(call("get_commit_diff", json!({})))
            .await
            .unwrap_err();

        assert!(
            error
                .to_string()
                .starts_with("invalid arguments for get_commit_diff: missing field `revision`")
        );
    }

    #[tokio::test]
    async fn returns_commit_message_as_a_string() {
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
        run_git(
            &[
                "commit",
                "--allow-empty",
                "--no-gpg-sign",
                "-m",
                "test message",
            ],
            repository.path(),
            console,
        )
        .await
        .unwrap();
        let executor =
            ExtractToolExecutor::new(Extractor::new(repository.path().to_path_buf(), console));

        let result = executor
            .execute(call("get_commit_message", json!({ "revision": "HEAD" })))
            .await
            .unwrap();

        assert_eq!(result, json!("test message"));
    }
}
