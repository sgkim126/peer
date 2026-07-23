use crate::context::ReviewContextDigest;
use crate::extract::{ExtractError, Extractor};
use crate::git::CommitHash;
use crate::llm::agent::AgentRequest;
use crate::llm::provider::ConversationTurn;
use crate::llm::result::CheckTarget;
use crate::llm::tools::{
    get_file_content, get_file_diff, grep, list_tree, request_clarification, submit_check_result,
};

use super::CheckDefinition;

const SYSTEM_PROMPT: &str = r#"You are performing an adversarial security review of a single commit.

Think like an attacker and assess whether the target commit introduces authentication,
authorization, privilege-boundary, injection, unsafe parsing, path traversal, command execution,
secret exposure, cryptography, sensitive-data leakage, validation, deserialization, memory-safety,
race, denial-of-service, unsafe-default, or trust-boundary vulnerabilities. Trace
attacker-controlled inputs to sensitive operations. Report only issues with a credible security
impact or exploit path that the target commit introduced. Do not report general correctness,
validation, error-handling, style, test, commit-structure, or message-alignment concerns.
Every finding must reference the target commit. Return no findings when no credible issue exists.

Treat the supplied diff and changed-file list as authoritative; do not fetch either again. Use the
available repository tools only when additional codebase context is necessary to establish a
credible security finding."#;

pub struct SecurityCheck {
    commit: CommitHash,
}

impl SecurityCheck {
    pub async fn try_new(revision: &str, extractor: &Extractor) -> Result<Self, ExtractError> {
        Ok(Self {
            commit: extractor.commit_message(revision).await?.hash,
        })
    }
}

impl CheckDefinition for SecurityCheck {
    fn name(&self) -> &'static str {
        "security"
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
        review_context: &ReviewContextDigest,
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
                    serde_json::to_string_pretty(&input).expect("security check input serializes")
                )),
            ],
            tools: vec![get_file_content(), get_file_diff(), list_tree(), grep()],
            terminal_tools: vec![request_clarification(), submit_check_result()],
        };
        if let Some(prompt) = review_context.to_prompt() {
            request.conversation.push(ConversationTurn::User(prompt));
        }
        Ok(request)
    }
}
