use std::fmt;

use futures_util::stream::{self, StreamExt};

use crate::extract::ExtractError;
use crate::llm::provider::ToolCall;

pub type ToolExecutionResult = Result<serde_json::Value, ToolExecutionError>;

#[derive(Debug)]
pub enum ToolExecutionError {
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
            Self::UnknownTool { .. } => None,
            Self::InvalidArguments { source, .. } => Some(source),
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

pub trait ToolExecutor {
    async fn execute(&self, call: ToolCall) -> ToolExecutionResult;

    async fn execute_all(&self, calls: Vec<ToolCall>) -> Vec<(String, ToolExecutionResult)> {
        let concurrency = std::thread::available_parallelism()
            .ok()
            .map(|cpus| cpus.get())
            .unwrap_or(1)
            .saturating_mul(2);

        stream::iter(calls)
            .map(|call| async {
                let call_id = call.id.clone();
                (call_id, self.execute(call).await)
            })
            .buffered(concurrency)
            .collect()
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::error::Error;

    struct EchoToolExecutor;

    impl ToolExecutor for EchoToolExecutor {
        async fn execute(&self, call: ToolCall) -> ToolExecutionResult {
            Ok(call.arguments)
        }
    }

    fn json_error() -> serde_json::Error {
        serde_json::from_str::<serde_json::Value>("{").unwrap_err()
    }

    #[tokio::test]
    async fn executor_returns_json_results() {
        let arguments = json!({ "revision": "HEAD" });
        let call = ToolCall {
            id: "call-1".to_string(),
            name: "get_commit_diff".to_string(),
            arguments: arguments.clone(),
            provider_state: None,
        };

        let result = EchoToolExecutor.execute(call).await.unwrap();

        assert_eq!(result, arguments);
    }

    #[tokio::test]
    async fn execute_all_returns_results_with_call_ids() {
        let executor = EchoToolExecutor;
        let results = executor
            .execute_all(vec![
                ToolCall {
                    id: "call-1".to_string(),
                    name: "get_commit_diff".to_string(),
                    arguments: json!({ "revision": "HEAD" }),
                    provider_state: None,
                },
                ToolCall {
                    id: "call-2".to_string(),
                    name: "get_commit_diff".to_string(),
                    arguments: json!({ "revision": "HEAD~1" }),
                    provider_state: None,
                },
            ])
            .await;

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "call-1");
        assert_eq!(
            results[0].1.as_ref().unwrap(),
            &json!({ "revision": "HEAD" })
        );
        assert_eq!(results[1].0, "call-2");
        assert_eq!(
            results[1].1.as_ref().unwrap(),
            &json!({ "revision": "HEAD~1" })
        );
    }

    #[test]
    fn unknown_tool_has_no_source() {
        let error = ToolExecutionError::UnknownTool {
            name: "missing".to_string(),
        };

        assert_eq!(error.to_string(), "unknown tool: missing");
        assert!(error.source().is_none());
    }

    #[test]
    fn invalid_arguments_include_the_tool_and_source() {
        let source = json_error();
        let source_message = source.to_string();
        let error = ToolExecutionError::InvalidArguments {
            tool: "get_commit_diff".to_string(),
            source,
        };

        assert_eq!(
            error.to_string(),
            format!("invalid arguments for get_commit_diff: {source_message}")
        );
        assert_eq!(error.source().unwrap().to_string(), source_message);
    }

    #[test]
    fn extract_errors_preserve_their_message_and_source() {
        let error = ToolExecutionError::from(ExtractError::InvalidTwoDotRange(
            "main...feature".to_string(),
        ));

        assert_eq!(error.to_string(), "main...feature is not a two-dot range");
        assert_eq!(
            error.source().unwrap().to_string(),
            "main...feature is not a two-dot range"
        );
        assert!(matches!(error, ToolExecutionError::Extract(_)));
    }

    #[test]
    fn serde_errors_convert_to_serialization_errors() {
        let source = json_error();
        let source_message = source.to_string();
        let error = ToolExecutionError::from(source);

        assert_eq!(
            error.to_string(),
            format!("cannot serialize tool result: {source_message}")
        );
        assert_eq!(error.source().unwrap().to_string(), source_message);
        assert!(matches!(error, ToolExecutionError::Serialization(_)));
    }
}
