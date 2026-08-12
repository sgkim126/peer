use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::git::CommitHash;
use crate::llm::LlmUsage;
use crate::pi::ReadTool;

use super::StageTarget;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StageKind {
    ReviewContext,
    CommitScope,
    CommitSequence,
    Size,
    Intent,
    Quality,
    Security,
}

impl StageKind {
    #[cfg_attr(not(test), expect(dead_code))]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReviewContext => "review_context",
            Self::CommitScope => "commit_scope",
            Self::CommitSequence => "commit_sequence",
            Self::Size => "size",
            Self::Intent => "intent",
            Self::Quality => "quality",
            Self::Security => "security",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StageRequest {
    pub system_prompt: String,
    pub prompt: String,
    pub read_tools: Vec<ReadTool>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClarificationQuestion {
    pub question: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum StageOutcome<R> {
    Completed {
        report: R,
    },
    Blocked {
        questions: Vec<ClarificationQuestion>,
    },
    Exhausted {
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[expect(dead_code)]
pub struct StageRun<R> {
    pub stage: StageKind,
    pub target: StageTarget,
    pub ordered_commits: Vec<CommitHash>,
    pub outcome: StageOutcome<R>,
    pub iterations: u32,
    pub usage: LlmUsage,
}

#[expect(dead_code)]
pub trait ReviewStage {
    type Report: Clone + Serialize + DeserializeOwned;

    fn kind(&self) -> StageKind;
    fn target(&self) -> StageTarget;
    fn expected_commits(&self) -> &[CommitHash];
    fn request(&self) -> StageRequest;
    fn validate_report(&self, report: &Self::Report) -> Result<(), String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_context_string_matches_serialized_name() {
        assert_eq!(StageKind::ReviewContext.as_str(), "review_context");
        assert_eq!(
            serde_json::to_value(StageKind::ReviewContext).unwrap(),
            "review_context",
        );
    }

    #[test]
    fn commit_scope_string_matches_serialized_name() {
        assert_eq!(StageKind::CommitScope.as_str(), "commit_scope");
        assert_eq!(
            serde_json::to_value(StageKind::CommitScope).unwrap(),
            "commit_scope",
        );
    }

    #[test]
    fn commit_sequence_string_matches_serialized_name() {
        assert_eq!(StageKind::CommitSequence.as_str(), "commit_sequence");
        assert_eq!(
            serde_json::to_value(StageKind::CommitSequence).unwrap(),
            "commit_sequence",
        );
    }

    #[test]
    fn size_string_matches_serialized_name() {
        assert_eq!(StageKind::Size.as_str(), "size");
        assert_eq!(serde_json::to_value(StageKind::Size).unwrap(), "size");
    }

    #[test]
    fn intent_string_matches_serialized_name() {
        assert_eq!(StageKind::Intent.as_str(), "intent");
        assert_eq!(serde_json::to_value(StageKind::Intent).unwrap(), "intent");
    }

    #[test]
    fn quality_string_matches_serialized_name() {
        assert_eq!(StageKind::Quality.as_str(), "quality");
        assert_eq!(serde_json::to_value(StageKind::Quality).unwrap(), "quality",);
    }

    #[test]
    fn security_string_matches_serialized_name() {
        assert_eq!(StageKind::Security.as_str(), "security");
        assert_eq!(
            serde_json::to_value(StageKind::Security).unwrap(),
            "security",
        );
    }
}
