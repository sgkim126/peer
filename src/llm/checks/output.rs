use serde::{Deserialize, Serialize};

use crate::error::PeerError;
use crate::extract::ExtractError;
use crate::llm::checks::CheckCommandError;
use crate::llm::checks::runner::CheckRunError;
use crate::llm::provider::{LlmCallError, ProviderCreationError};
use crate::llm::result::{CheckOutcome, CheckResult};

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CheckCommandOutput {
    Success { data: CheckOutcome },
    Error { error: CheckCommandErrorOutput },
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CheckCommandErrorOutput {
    pub code: ErrorCode,
    pub message: String,
    pub is_retryable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    ConfigInvalid,
    GitCommandFailed,
    Internal,
    InvalidArgument,
    LlmRequestFailed,
}

impl From<CheckCommandError> for CheckCommandErrorOutput {
    fn from(error: CheckCommandError) -> Self {
        let message = error.to_string();
        let (code, is_retryable) = match error {
            CheckCommandError::Config(error) => peer_error_classification(&error),
            CheckCommandError::InvalidConfidence(_) => (ErrorCode::ConfigInvalid, false),
            CheckCommandError::Provider(error) => provider_error_classification(&error),
            CheckCommandError::Run(error) => check_run_error_classification(&error),
        };

        Self {
            code,
            message,
            is_retryable,
        }
    }
}

impl From<Result<CheckResult, CheckCommandError>> for CheckCommandOutput {
    fn from(result: Result<CheckResult, CheckCommandError>) -> Self {
        match result {
            Ok(result) => Self::success(result),
            Err(error) => Self::error(CheckCommandErrorOutput::from(error)),
        }
    }
}

fn peer_error_classification(error: &PeerError) -> (ErrorCode, bool) {
    match error {
        PeerError::Internal { .. } => (ErrorCode::Internal, false),
        PeerError::InvalidConfig { .. } => (ErrorCode::ConfigInvalid, false),
        PeerError::Git(_) => (ErrorCode::GitCommandFailed, false),
    }
}

fn provider_error_classification(error: &ProviderCreationError) -> (ErrorCode, bool) {
    match error {
        ProviderCreationError::Unsupported { .. } => (ErrorCode::ConfigInvalid, false),
        ProviderCreationError::Initialization(error) => llm_error_classification(error),
    }
}

fn check_run_error_classification(error: &CheckRunError) -> (ErrorCode, bool) {
    match error {
        CheckRunError::Preparation(error) => extract_error_classification(error),
        CheckRunError::LlmCall(error) => llm_error_classification(error),
    }
}

fn extract_error_classification(error: &ExtractError) -> (ErrorCode, bool) {
    match error {
        ExtractError::Git(_) => (ErrorCode::GitCommandFailed, false),
        ExtractError::InvalidTwoDotRange(_) | ExtractError::InvalidRevision(_) => {
            (ErrorCode::InvalidArgument, false)
        }
    }
}

fn llm_error_classification(error: &LlmCallError) -> (ErrorCode, bool) {
    (
        ErrorCode::LlmRequestFailed,
        matches!(error, LlmCallError::Transient { .. }),
    )
}

impl CheckCommandOutput {
    pub fn success(data: CheckResult) -> Self {
        Self::Success {
            data: CheckOutcome::success(data),
        }
    }

    pub fn error(error: CheckCommandErrorOutput) -> Self {
        Self::Error { error }
    }

    pub fn as_outcome(&self) -> Result<&CheckOutcome, &CheckCommandErrorOutput> {
        match self {
            Self::Success { data } => Ok(data),
            Self::Error { error } => Err(error),
        }
    }

    pub fn as_result(&self) -> Result<&CheckResult, &CheckCommandErrorOutput> {
        self.as_outcome()?.as_success().ok_or_else(|| match self {
            Self::Error { error } => error,
            Self::Success { .. } => {
                unreachable!("successful check command output contains no successful result")
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::error::PeerError;
    use crate::extract::ExtractError;
    use crate::git::CommitHash;
    use crate::git::GitError;
    use crate::llm::checks::CheckCommandError;
    use crate::llm::checks::runner::CheckRunError;
    use crate::llm::confidence::Confidence;
    use crate::llm::provider::LlmCallError;
    use crate::llm::result::{CheckTarget, CheckUsage};

    fn check_result() -> CheckResult {
        CheckResult {
            check: "size".to_string(),
            target: CheckTarget::Commit(CommitHash::new("abc1234").unwrap()),
            summary: "The commit is appropriately sized.".to_string(),
            findings: Vec::new(),
            confidence: Confidence::try_from(0.9).unwrap(),
            iterations: 1,
            is_exhausted: false,
            exhaustion_reason: None,
            usage: CheckUsage {
                input_tokens: 100,
                output_tokens: 20,
                cost_usd: 0.001,
                model: "test-model".to_string(),
            },
        }
    }

    #[test]
    fn success_output_wraps_check_result_in_data() {
        let value = serde_json::to_value(CheckCommandOutput::from(Ok(check_result()))).unwrap();

        assert_eq!(
            value,
            json!({
                "status": "success",
                "data": {
                    "status": "success",
                    "check": {
                        "check": "size",
                        "target": "abc1234",
                        "summary": "The commit is appropriately sized.",
                        "findings": [],
                        "confidence": 0.9,
                        "iterations": 1,
                        "is_exhausted": false,
                        "exhaustion_reason": null,
                        "usage": {
                            "input_tokens": 100,
                            "output_tokens": 20,
                            "cost_usd": 0.001,
                            "model": "test-model"
                        }
                    }
                }
            })
        );
    }

    #[test]
    fn error_output_wraps_error_payload() {
        let error = CheckCommandError::Run(CheckRunError::LlmCall(LlmCallError::Transient {
            message: "request timed out".to_string(),
            source: Box::new(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "request timed out",
            )),
        }));
        let value = serde_json::to_value(CheckCommandOutput::from(Err(error))).unwrap();

        assert_eq!(value["status"], "error");
        assert_eq!(value["error"]["code"], "llm_request_failed");
        assert_eq!(
            value["error"]["message"],
            "transient LLM call failure: request timed out"
        );
        assert_eq!(value["error"]["is_retryable"], true);
        assert!(value.get("data").is_none());
    }

    #[test]
    fn config_error_is_non_retryable() {
        let error = CheckCommandError::Config(PeerError::InvalidConfig {
            message: "invalid configuration".to_string(),
            source: None,
        });

        let output = CheckCommandErrorOutput::from(error);

        assert_eq!(output.code, ErrorCode::ConfigInvalid);
        assert_eq!(output.message, "invalid configuration");
        assert!(!output.is_retryable);
    }

    #[test]
    fn transient_llm_error_is_retryable() {
        let error = CheckCommandError::Run(CheckRunError::LlmCall(LlmCallError::Transient {
            message: "request timed out".to_string(),
            source: Box::new(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "request timed out",
            )),
        }));

        let output = CheckCommandErrorOutput::from(error);

        assert_eq!(output.code, ErrorCode::LlmRequestFailed);
        assert_eq!(
            output.message,
            "transient LLM call failure: request timed out"
        );
        assert!(output.is_retryable);
    }

    #[test]
    fn permanent_llm_error_is_non_retryable() {
        let error = CheckCommandError::Run(CheckRunError::LlmCall(LlmCallError::Permanent {
            message: "invalid API key".to_string(),
            source: Box::new(std::io::Error::other("unauthorized")),
        }));

        let output = CheckCommandErrorOutput::from(error);

        assert_eq!(output.code, ErrorCode::LlmRequestFailed);
        assert!(!output.is_retryable);
    }

    #[test]
    fn preparation_git_error_uses_git_error_code() {
        let error = CheckCommandError::Run(CheckRunError::Preparation(ExtractError::Git(
            GitError::NonZeroExit {
                status: 128,
                stderr: "unknown revision".to_string(),
            },
        )));

        let output = CheckCommandErrorOutput::from(error);

        assert_eq!(output.code, ErrorCode::GitCommandFailed);
        assert!(!output.is_retryable);
    }

    #[test]
    fn invalid_preparation_input_uses_invalid_argument_code() {
        let error = CheckCommandError::Run(CheckRunError::Preparation(
            ExtractError::InvalidTwoDotRange("HEAD".to_string()),
        ));

        let output = CheckCommandErrorOutput::from(error);

        assert_eq!(output.code, ErrorCode::InvalidArgument);
        assert!(!output.is_retryable);
    }
}
