use crate::extract::{CommitList, CommitMessage, ExtractError, Extractor};
use crate::llm::provider::ConversationTurn;

use super::{CheckDefinition, PreparedCheck, PreparedCheckTarget, all_tools, output_schema};

const SYSTEM_PROMPT: &str = r#"You are reviewing a commit series for coherence.

Assess whether:
1. The commits form a clear, logically ordered story.
2. Fixup or follow-up commits should be squashed into earlier commits.
3. Any intermediate commit appears incomplete or likely to break builds or tests.
4. Responsibilities are split across commits in a confusing or unsafe way.
5. Commit messages accurately communicate the progression of the series.

The required commit list is ordered from oldest to newest. Findings must reference the specific commit responsible for the issue. Use tools to inspect diffs, changed files, or file contents when needed. Return no findings when the series is coherent and each intermediate commit stands on its own."#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoherenceCheck {
    range: String,
}

impl CoherenceCheck {
    pub fn new(range: String) -> Self {
        Self { range }
    }
}

impl CheckDefinition for CoherenceCheck {
    fn name(&self) -> &'static str {
        "coherence"
    }

    async fn prepare(
        &self,
        extractor: &Extractor,
        _review_context: &crate::llm::context::ReviewContext,
    ) -> Result<PreparedCheck, ExtractError> {
        let commit_list = extractor.commit_list(&self.range).await?;
        let mut messages = Vec::with_capacity(commit_list.commits.len());

        for commit in &commit_list.commits {
            messages.push(extractor.commit_message(commit.as_ref()).await?);
        }

        Ok(build_prepared_check(commit_list, messages))
    }
}

fn build_prepared_check(commit_list: CommitList, messages: Vec<CommitMessage>) -> PreparedCheck {
    let commits = commit_list.commits;
    let ordered_commits = messages
        .into_iter()
        .enumerate()
        .map(|(index, message)| {
            format!(
                "{}. {}\n{}",
                index + 1,
                message.hash,
                indent(&message.message)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    PreparedCheck {
        conversation: vec![
            ConversationTurn::System(SYSTEM_PROMPT.to_string()),
            ConversationTurn::User(format!(
                "Review range {}.\n\nCommits (oldest to newest):\n{}",
                commit_list.range, ordered_commits
            )),
        ],
        tools: all_tools(),
        output_schema: output_schema(),
        target: PreparedCheckTarget::Range {
            revision: commit_list.range,
            commits,
        },
    }
}

fn indent(value: &str) -> String {
    value
        .lines()
        .map(|line| format!("   {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::CommitHash;
    use crate::llm::result::{CheckOutput, CheckTarget};

    fn hash(value: &str) -> CommitHash {
        CommitHash::new(value).unwrap()
    }

    fn prepared_check() -> PreparedCheck {
        build_prepared_check(
            CommitList {
                range: "base123..tip4567".to_string(),
                commits: vec![hash("abc1234"), hash("def5678")],
            },
            vec![
                CommitMessage {
                    hash: hash("abc1234"),
                    message: "Add parser".to_string(),
                },
                CommitMessage {
                    hash: hash("def5678"),
                    message: "Fix parser edge case".to_string(),
                },
            ],
        )
    }

    #[test]
    fn name_is_coherence() {
        assert_eq!(
            CoherenceCheck::new("HEAD~2..HEAD".to_string()).name(),
            "coherence"
        );
    }

    #[test]
    fn prompt_contains_numbered_oldest_to_newest_commits() {
        let prepared = prepared_check();

        let ConversationTurn::System(system) = &prepared.conversation[0] else {
            panic!("expected system prompt");
        };
        assert!(system.contains("commit series for coherence"));

        let ConversationTurn::User(user) = &prepared.conversation[1] else {
            panic!("expected required data");
        };
        let first = user.find("1. abc1234").unwrap();
        let second = user.find("2. def5678").unwrap();
        assert!(first < second);
        assert!(user.contains("Add parser"));
        assert!(user.contains("Fix parser edge case"));
    }

    #[test]
    fn prepared_check_preserves_original_range_and_commits() {
        let prepared = prepared_check();

        assert_eq!(
            prepared.result_target(),
            CheckTarget::Range("base123..tip4567".to_string())
        );
        assert_eq!(prepared.tools.len(), 5);
        assert_eq!(
            prepared.output_schema["required"],
            serde_json::json!(["summary", "findings", "confidence"])
        );
    }

    #[test]
    fn prepared_check_validates_findings_against_range_commits() {
        let prepared = prepared_check();
        let in_range: CheckOutput = serde_json::from_value(serde_json::json!({
            "summary": "fixup should be squashed",
            "findings": [{
                "commit": "def5678",
                "severity": "medium",
                "message": "This commit only fixes the immediately preceding commit."
            }],
            "confidence": 0.9
        }))
        .unwrap();
        let outside_range: CheckOutput = serde_json::from_value(serde_json::json!({
            "summary": "invalid target",
            "findings": [{
                "commit": "9876abc",
                "severity": "medium",
                "message": "Outside the reviewed range."
            }],
            "confidence": 0.9
        }))
        .unwrap();

        assert!(prepared.validate_output(&in_range).is_ok());
        assert!(prepared.validate_output(&outside_range).is_err());
    }
}
