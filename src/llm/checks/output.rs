use serde::{Deserialize, Serialize};

use crate::llm::result::CheckResult;

#[derive(Debug, Deserialize, Serialize)]
#[allow(dead_code)]
pub struct CheckCommandOutput {
    #[serde(flatten)]
    outcome: CheckCommandOutcome,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
#[allow(dead_code)]
enum CheckCommandOutcome {
    Success { data: CheckResult },
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

impl CheckCommandOutput {
    #[allow(dead_code)]
    pub fn success(data: CheckResult) -> Self {
        Self {
            outcome: CheckCommandOutcome::Success { data },
        }
    }

    #[allow(dead_code)]
    pub fn error(error: CheckCommandErrorOutput) -> Self {
        Self {
            outcome: CheckCommandOutcome::Error { error },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::CommitHash;
    use crate::llm::confidence::Confidence;
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
        let value = serde_json::to_value(CheckCommandOutput::success(check_result())).unwrap();

        assert_eq!(value["status"], "success");
        assert_eq!(value["data"]["check"], "size");
        assert!(value.get("error").is_none());
    }

    #[test]
    fn error_output_wraps_error_payload() {
        let value = serde_json::to_value(CheckCommandOutput::error(CheckCommandErrorOutput {
            code: ErrorCode::LlmRequestFailed,
            message: "request timed out".to_string(),
            is_retryable: true,
        }))
        .unwrap();

        assert_eq!(value["status"], "error");
        assert_eq!(value["error"]["code"], "llm_request_failed");
        assert_eq!(value["error"]["message"], "request timed out");
        assert_eq!(value["error"]["is_retryable"], true);
        assert!(value.get("data").is_none());
    }
}
