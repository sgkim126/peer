use crate::context::ReviewContext;
use crate::extract::{CommitList, ExtractError, Extractor};
use crate::git::CommitHash;
use crate::llm::agent::AgentRequest;
use crate::llm::provider::ConversationTurn;
use crate::llm::result::CheckTarget;
use crate::llm::tools::{
    get_changed_files, get_commit_diff, request_clarification, submit_check_result,
};

use super::CheckDefinition;

const SYSTEM_PROMPT: &str = r#"You are reviewing a commit series for coherence.

Assess commit ordering, dependencies, narrative flow, unnecessary fixups or reversions, confusing
splits of one logical change, mixed responsibilities, and whether the message sequence clearly
communicates work progression. Do not report individual correctness, style, tests, security, or
message-to-diff alignment. Every finding must reference the commit responsible for the
series-level issue. Return no findings when the series forms a clear progression.

Treat the supplied ordered commit messages as authoritative; do not fetch them again. Use diff or
changed-file lookup only when additional evidence is necessary to establish a relationship between
commits."#;

pub struct CoherenceCheck {
    commits: CommitList,
}

impl CoherenceCheck {
    pub async fn try_new(range: &str, extractor: &Extractor) -> Result<Self, ExtractError> {
        Ok(Self {
            commits: extractor.commit_list(range).await?,
        })
    }
}

impl CheckDefinition for CoherenceCheck {
    fn name(&self) -> &'static str {
        "coherence"
    }

    fn target(&self) -> CheckTarget {
        CheckTarget::Range(self.commits.range.clone())
    }

    fn expected_commits(&self) -> &[CommitHash] {
        &self.commits.commits
    }

    async fn agent_request(
        &self,
        extractor: &Extractor,
        model: &str,
        review_context: &ReviewContext,
    ) -> Result<AgentRequest, ExtractError> {
        let mut entries = Vec::with_capacity(self.commits.commits.len());
        for (index, commit) in self.commits.commits.iter().enumerate() {
            let message = extractor.commit_message(commit.as_ref()).await?;
            entries.push(format!(
                "{}. {}\n{}",
                index + 1,
                message.hash,
                indent(&message.message)
            ));
        }
        let mut request = AgentRequest {
            model: model.to_string(),
            conversation: vec![
                ConversationTurn::System(SYSTEM_PROMPT.to_string()),
                ConversationTurn::User(format!(
                    "Review range {}.\n\nCommits (oldest to newest):\n{}",
                    self.commits.range,
                    entries.join("\n\n")
                )),
            ],
            tools: vec![get_commit_diff(), get_changed_files()],
            terminal_tools: vec![request_clarification(), submit_check_result()],
        };
        if let Some(prompt) = review_context.to_prompt() {
            request.conversation.push(ConversationTurn::User(prompt));
        }
        Ok(request)
    }
}

fn indent(value: &str) -> String {
    value
        .lines()
        .map(|line| format!("   {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}
