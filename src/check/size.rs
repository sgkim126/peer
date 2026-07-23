use crate::context::ReviewContext;
use crate::extract::{ExtractError, Extractor};
use crate::git::CommitHash;
use crate::llm::agent::AgentRequest;
use crate::llm::provider::ConversationTurn;
use crate::llm::result::CheckTarget;
use crate::llm::tools::{get_file_content, request_clarification, submit_check_result};

use super::CheckDefinition;

const SYSTEM_PROMPT: &str = r#"You are reviewing a single commit for scope and atomicity.

Assess whether the commit mixes unrelated responsibilities, combines independent refactoring and
behavior changes, is too broad to review or revert safely, or omits an obvious directly related
change needed to make its structural unit whole. Do not assess correctness, style, tests,
security, or whether the commit message describes the diff. A large commit alone is not a finding.
Every finding must reference the target commit. Return no findings when the commit is coherent.

Treat the supplied diff and changed-file list as authoritative. Do not request or use commit-message
data. Use file-content lookup only when additional surrounding context is necessary to judge the
change's structural boundaries."#;

pub struct SizeCheck {
    commit: CommitHash,
}

impl SizeCheck {
    pub async fn try_new(revision: &str, extractor: &Extractor) -> Result<Self, ExtractError> {
        Ok(Self {
            commit: extractor.commit_message(revision).await?.hash,
        })
    }
}

impl CheckDefinition for SizeCheck {
    fn name(&self) -> &'static str {
        "size"
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
        review_context: &ReviewContext,
    ) -> Result<AgentRequest, ExtractError> {
        let diff = extractor.commit_diff(self.commit.as_ref()).await?;
        let files = extractor.commit_files(self.commit.as_ref()).await?;
        let input = serde_json::json!({
            "target_commit": self.commit,
            "changed_files": files.files,
            "diff": diff.diff,
        });
        let mut request = AgentRequest {
            model: model.to_string(),
            conversation: vec![
                ConversationTurn::System(SYSTEM_PROMPT.to_string()),
                ConversationTurn::User(format!(
                    "Review the following required commit data:\n{}",
                    serde_json::to_string_pretty(&input).expect("size check input serializes")
                )),
            ],
            tools: vec![get_file_content()],
            terminal_tools: vec![request_clarification(), submit_check_result()],
        };
        if let Some(prompt) = review_context.to_prompt() {
            request.conversation.push(ConversationTurn::User(prompt));
        }
        Ok(request)
    }
}
