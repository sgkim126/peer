use crate::extract::{CommitDiff, CommitMessage, ExtractError, Extractor};
use crate::llm::provider::ConversationTurn;

use super::{CheckDefinition, PreparedCheck, PreparedCheckTarget, all_tools, output_schema};

const SYSTEM_PROMPT: &str = r#"You are reviewing a single commit for alignment between its stated intent and its actual changes.

Assess whether:
1. The commit message accurately describes the behavior and scope of the diff.
2. The diff contains unrelated or undocumented changes.
3. The message claims work that the diff does not implement.
4. Important user-visible, compatibility, migration, or operational effects are omitted from the message.

Use the required commit data supplied by the user. Use tools only when additional context is needed. Every finding must reference the target commit. Return no findings when the message and diff are aligned."#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntentCheck {
    revision: String,
}

impl IntentCheck {
    pub fn new(revision: String) -> Self {
        Self { revision }
    }
}

impl CheckDefinition for IntentCheck {
    fn name(&self) -> &'static str {
        "intent"
    }

    async fn prepare(&self, extractor: &Extractor) -> Result<PreparedCheck, ExtractError> {
        let message = extractor.commit_message(&self.revision).await?;
        let diff = extractor.commit_diff(message.hash.as_ref()).await?;

        Ok(build_prepared_check(message, diff))
    }
}

fn build_prepared_check(message: CommitMessage, diff: CommitDiff) -> PreparedCheck {
    let target = message.hash.clone();
    let required_data = serde_json::json!({
        "target_commit": target,
        "commit_message": message.message,
        "diff": diff.diff,
    });

    PreparedCheck {
        conversation: vec![
            ConversationTurn::System(SYSTEM_PROMPT.to_string()),
            ConversationTurn::User(format!(
                "Review the following required commit data:\n{}",
                serde_json::to_string_pretty(&required_data)
                    .expect("serializing intent check input cannot fail")
            )),
        ],
        tools: all_tools(),
        output_schema: output_schema(),
        target: PreparedCheckTarget::Commit(message.hash),
    }
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
        let target = hash("abc1234");
        build_prepared_check(
            CommitMessage {
                hash: target.clone(),
                message: "Reject empty API keys".to_string(),
            },
            CommitDiff {
                hash: target,
                diff: "diff --git a/src/config.rs b/src/config.rs\n+validate_api_key();"
                    .to_string(),
            },
        )
    }

    #[test]
    fn name_is_intent() {
        assert_eq!(IntentCheck::new("HEAD".to_string()).name(), "intent");
    }

    #[test]
    fn prepared_conversation_contains_message_and_diff() {
        let prepared = prepared_check();

        let ConversationTurn::System(system) = &prepared.conversation[0] else {
            panic!("expected system prompt");
        };
        assert!(system.contains("stated intent"));

        let ConversationTurn::User(user) = &prepared.conversation[1] else {
            panic!("expected required data");
        };
        assert!(user.contains("\"target_commit\": \"abc1234\""));
        assert!(user.contains("\"commit_message\": \"Reject empty API keys\""));
        assert!(user.contains("validate_api_key"));
    }

    #[test]
    fn prepared_check_uses_commit_target_and_common_inputs() {
        let prepared = prepared_check();

        assert_eq!(
            prepared.result_target(),
            CheckTarget::Commit(hash("abc1234"))
        );
        assert_eq!(prepared.tools.len(), 5);
        assert_eq!(
            prepared.output_schema["required"],
            serde_json::json!(["summary", "findings", "confidence"])
        );
    }

    #[test]
    fn prepared_check_validates_finding_commit() {
        let prepared = prepared_check();
        let matching: CheckOutput = serde_json::from_value(serde_json::json!({
            "summary": "message omits behavior",
            "findings": [{
                "commit": "abc1234",
                "severity": "medium",
                "message": "The diff also changes fallback behavior."
            }],
            "confidence": 0.9
        }))
        .unwrap();
        let wrong: CheckOutput = serde_json::from_value(serde_json::json!({
            "summary": "message omits behavior",
            "findings": [{
                "commit": "def5678",
                "severity": "medium",
                "message": "Wrong target."
            }],
            "confidence": 0.9
        }))
        .unwrap();

        assert!(prepared.validate_output(&matching).is_ok());
        assert!(prepared.validate_output(&wrong).is_err());
    }
}
