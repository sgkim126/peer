use std::fmt;

use serde::{Deserialize, Serialize};

use crate::git::CommitHash;
use crate::llm::LlmUsage;

use super::contract::ClarificationQuestion;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StageFailure {
    Exhausted {
        reason: String,
    },
    ClarificationRequired {
        questions: Vec<ClarificationQuestion>,
    },
}

impl fmt::Display for StageFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exhausted { reason } => f.write_str(reason),
            Self::ClarificationRequired { .. } => f.write_str("clarification required"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct FileLocation {
    pub file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Finding {
    pub commit: CommitHash,
    pub severity: Severity,
    pub message: String,
    #[serde(flatten)]
    pub location: Option<FileLocation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum StageTarget {
    Commit(CommitHash),
    Range { from: CommitHash, to: CommitHash },
}

impl fmt::Display for StageTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Commit(commit) => commit.fmt(f),
            Self::Range { from, to } => write!(f, "{from}..{to}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct StageResult {
    pub stage: String,
    pub target: StageTarget,
    /// Target commits in review order, from oldest to newest.
    pub ordered_commits: Vec<CommitHash>,
    pub summary: String,
    pub findings: Vec<Finding>,
    pub iterations: u32,
    pub failure: Option<StageFailure>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_usage: Option<LlmUsage>,
    pub usage: LlmUsage,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result() -> StageResult {
        StageResult {
            stage: "quality".into(),
            target: StageTarget::Commit(CommitHash::new("abc1234").unwrap()),
            ordered_commits: vec![CommitHash::new("abc1234").unwrap()],
            summary: "No issues found.".into(),
            findings: Vec::new(),
            iterations: 1,
            failure: None,
            context_usage: None,
            usage: LlmUsage::zero("test-model"),
        }
    }

    #[test]
    fn serialization_omits_missing_context_usage() {
        let value = serde_json::to_value(result()).unwrap();

        assert_eq!(value.get("context_usage"), None);
    }

    #[test]
    fn deserialization_defaults_missing_context_usage_to_none() {
        let mut value = serde_json::to_value(result()).unwrap();
        value.as_object_mut().unwrap().remove("context_usage");

        let decoded: StageResult = serde_json::from_value(value).unwrap();

        assert_eq!(decoded.context_usage, None);
    }
}
