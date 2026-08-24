use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

use crate::git::CommitHash;
use crate::llm::LlmUsage;
use crate::pi::ReadTool;

use super::StageTarget;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StageKind {
    ReviewContext,
    Knowledge,
    Quality,
    Security,
}

impl StageKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReviewContext => "review_context",
            Self::Knowledge => "knowledge",
            Self::Quality => "quality",
            Self::Security => "security",
        }
    }
}

impl FromStr for StageKind {
    type Err = ();

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        match name {
            "review_context" => Ok(Self::ReviewContext),
            "knowledge" => Ok(Self::Knowledge),
            "quality" => Ok(Self::Quality),
            "security" => Ok(Self::Security),
            _ => Err(()),
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
pub struct StageRun<R> {
    pub stage: StageKind,
    pub target: StageTarget,
    pub ordered_commits: Vec<CommitHash>,
    pub outcome: StageOutcome<R>,
    pub iterations: u32,
    pub usage: LlmUsage,
}

pub trait ReviewStage {
    type Report: Serialize + DeserializeOwned;

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
    fn knowledge_string_matches_serialized_name() {
        assert_eq!(StageKind::Knowledge.as_str(), "knowledge");
        assert_eq!(
            serde_json::to_value(StageKind::Knowledge).unwrap(),
            "knowledge"
        );
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
