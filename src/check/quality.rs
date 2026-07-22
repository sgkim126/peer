use crate::extract::{ExtractError, Extractor};
use crate::git::CommitHash;
use crate::llm::agent::AgentRequest;
use crate::llm::provider::ConversationTurn;
use crate::llm::result::CheckTarget;
use crate::llm::tools::{
    get_file_content, get_file_diff, grep, list_tree, request_clarification, submit_check_result,
};

use super::CheckDefinition;

const SYSTEM_PROMPT: &str = r#"You are reviewing a single commit for general code quality.

Assess the changed code for correctness issues and likely bugs, misuse of language or project
idioms, unclear or fragile implementation, error-handling and boundary-condition problems,
maintainability concerns, and missing tests for introduced behavior. Focus on concrete issues
introduced by the target commit. Do not report security vulnerabilities, commit structure or size,
message-to-diff alignment, unrelated pre-existing problems, or subjective style preferences.
Every finding must reference the target commit. Return no findings when no actionable issue exists.

Treat the supplied diff and changed-file list as authoritative; do not fetch either again. Use the
available repository tools only when additional codebase context is necessary to establish a
concrete finding."#;

pub struct QualityCheck {
    commit: CommitHash,
}

impl QualityCheck {
    pub async fn try_new(revision: &str, extractor: &Extractor) -> Result<Self, ExtractError> {
        Ok(Self {
            commit: extractor.commit_message(revision).await?.hash,
        })
    }
}

impl CheckDefinition for QualityCheck {
    fn name(&self) -> &'static str {
        "quality"
    }

    fn target(&self) -> CheckTarget {
        CheckTarget::Commit(self.commit.clone())
    }

    fn expected_commits(&self) -> &[CommitHash] {
        std::slice::from_ref(&self.commit)
    }

    async fn agent_request(
        &self,
        extractor: &Extractor,
        model: &str,
    ) -> Result<AgentRequest, ExtractError> {
        let diff = extractor.commit_diff(self.commit.as_ref()).await?;
        let files = extractor.commit_files(self.commit.as_ref()).await?;
        let input = serde_json::json!({
            "target_commit": self.commit,
            "changed_files": files.files,
            "diff": diff.diff,
        });
        Ok(AgentRequest {
            model: model.to_string(),
            conversation: vec![
                ConversationTurn::System(SYSTEM_PROMPT.to_string()),
                ConversationTurn::User(format!(
                    "Review the following required commit data:\n{}",
                    serde_json::to_string_pretty(&input).expect("quality check input serializes")
                )),
            ],
            tools: vec![
                get_file_content(),
                get_file_diff(),
                list_tree(),
                grep(),
                request_clarification(),
                submit_check_result(),
            ],
        })
    }
}
