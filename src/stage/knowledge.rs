use serde::{Deserialize, Serialize};

use crate::git::CommitHash;
use crate::stage::FileLocation;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeQuestionCategory {
    Objective,
    ExpectedBehavior,
    Constraint,
    Rationale,
    Tradeoff,
    Operations,
    Verification,
    ChangeStructure,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeQuestion {
    pub category: KnowledgeQuestionCategory,
    pub question: String,
    pub evidence: String,
    pub why_it_matters: String,
    pub related_commits: Vec<CommitHash>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<KnowledgeLocation>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeLocation {
    pub commit: CommitHash,
    #[serde(flatten)]
    pub file: FileLocation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StructuralRecommendationKind {
    SplitPullRequest,
    ExtractPrerequisite,
    ReorderCommits,
    SplitCommit,
    MoveChange,
    MergeSquash,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StructuralRecommendation {
    pub kind: StructuralRecommendationKind,
    pub message: String,
    pub rationale: String,
    pub related_commits: Vec<CommitHash>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[expect(dead_code)]
pub struct KnowledgeReport {
    pub summary: String,
    pub questions: Vec<KnowledgeQuestion>,
    pub recommendations: Vec<StructuralRecommendation>,
}
