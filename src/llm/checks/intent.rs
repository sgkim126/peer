use serde::Serialize;

use crate::cache::CacheKey;
use crate::extract::{CommitDiff, CommitMessage, ExtractError, Extractor};
use crate::git::CommitHash;
use crate::llm::context::ReviewContext;
use crate::llm::provider::ConversationTurn;

use super::{CheckDefinition, PreparedCheck, PreparedCheckTarget, all_tools, output_schema};

const SYSTEM_PROMPT: &str = r#"You are reviewing a single commit for alignment between its stated intent and its actual changes.

Assess whether:
1. The commit message accurately describes the behavior and scope of the diff.
2. The diff contains unrelated or undocumented changes.
3. The message claims work that the diff does not implement.
4. Important user-visible, compatibility, migration, or operational effects are omitted from the message.

Your scope is only whether the commit message and its diff agree. Do not assess code
correctness, bugs, implementation quality, test coverage, security impact, or whether the
commit should be split; these are outside the scope of this check. Do not report a vague or
stylistically weak message when it accurately describes the change.

Use the required commit data supplied by the user. Use tools only when additional context is needed to compare the message with the diff. Every finding must reference the target commit. Return no findings when the message and diff are aligned."#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntentCheck {
    commit: CommitHash,
}

impl IntentCheck {
    pub async fn try_new(revision: &str, extractor: &Extractor) -> Result<Self, ExtractError> {
        Ok(Self {
            commit: extractor.resolve_commit(revision).await?,
        })
    }
}

impl CheckDefinition for IntentCheck {
    fn name(&self) -> &'static str {
        "intent"
    }

    fn cache_key(&self, provider: &str, model: &str, review_context: &ReviewContext) -> CacheKey {
        let params = IntentCheckCacheParams {
            commit: &self.commit,
            review_context,
        };

        CacheKey::from_params(self.name(), provider, model, &params)
            .expect("serializing intent check cache params cannot fail")
    }

    async fn prepare(
        &self,
        extractor: &Extractor,
        review_context: &ReviewContext,
    ) -> Result<PreparedCheck, ExtractError> {
        let message = extractor.commit_message(self.commit.as_ref()).await?;
        let diff = extractor.commit_diff(self.commit.as_ref()).await?;

        Ok(build_prepared_check(message, diff, review_context))
    }
}

#[derive(Debug, Serialize)]
struct IntentCheckCacheParams<'a> {
    commit: &'a CommitHash,
    review_context: &'a ReviewContext,
}

fn build_prepared_check(
    message: CommitMessage,
    diff: CommitDiff,
    review_context: &ReviewContext,
) -> PreparedCheck {
    let target = message.hash.clone();
    let required_data = serde_json::json!({
        "target_commit": target,
        "commit_message": message.message,
        "diff": diff.diff,
    });
    let mut user_prompt = format!(
        "Review the following required commit data:\n{}",
        serde_json::to_string_pretty(&required_data)
            .expect("serializing intent check input cannot fail")
    );
    review_context.append_to_prompt(&mut user_prompt);

    PreparedCheck {
        conversation: vec![
            ConversationTurn::System(SYSTEM_PROMPT.to_string()),
            ConversationTurn::User(user_prompt),
        ],
        tools: all_tools(),
        output_schema: output_schema(),
        target: PreparedCheckTarget::Commit(message.hash),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            &ReviewContext::default(),
        )
    }

    #[test]
    fn name_is_intent() {
        let check = IntentCheck {
            commit: hash("abc1234"),
        };

        assert_eq!(check.name(), "intent");
    }

    #[test]
    fn prepared_conversation_contains_message_and_diff() {
        let prepared = prepared_check();

        let ConversationTurn::System(system) = &prepared.conversation[0] else {
            panic!("expected system prompt");
        };
        assert!(system.contains("stated intent"));
        assert!(system.contains("whether the commit message and its diff agree"));
        assert!(system.contains("Your scope is only whether the commit message"));

        let ConversationTurn::User(user) = &prepared.conversation[1] else {
            panic!("expected required data");
        };
        assert!(user.contains("\"target_commit\": \"abc1234\""));
        assert!(user.contains("\"commit_message\": \"Reject empty API keys\""));
        assert!(user.contains("validate_api_key"));
        assert!(!user.contains("Review context:"));
    }

    #[test]
    fn prepared_conversation_contains_review_context_when_present() {
        let target = hash("abc1234");
        let prepared = build_prepared_check(
            CommitMessage {
                hash: target.clone(),
                message: "Reject empty API keys".to_string(),
            },
            CommitDiff {
                hash: target,
                diff: "diff --git a/src/config.rs b/src/config.rs\n+validate_api_key();"
                    .to_string(),
            },
            &ReviewContext {
                title: Some("Reject invalid config".to_string()),
                body_summary: Some("Adds validation to config loading.".to_string()),
                comments_summary: Some("Reviewer asked about empty API keys.".to_string()),
            },
        );

        let ConversationTurn::User(user) = &prepared.conversation[1] else {
            panic!("expected required data");
        };
        assert!(user.contains("Review context:"));
        assert!(user.contains("Title:\nReject invalid config"));
        assert!(user.contains("Body summary:\nAdds validation to config loading."));
        assert!(user.contains("Comments summary:\nReviewer asked about empty API keys."));
    }

    #[test]
    fn prepared_check_uses_commit_target_and_common_inputs() {
        let prepared = prepared_check();

        assert_eq!(
            prepared.result_target(),
            CheckTarget::Commit(hash("abc1234"))
        );
        assert_eq!(prepared.tools.len(), 6);
        assert_eq!(
            prepared.output_schema["required"],
            serde_json::json!(["summary", "findings"])
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
            }]
        }))
        .unwrap();
        let wrong: CheckOutput = serde_json::from_value(serde_json::json!({
            "summary": "message omits behavior",
            "findings": [{
                "commit": "def5678",
                "severity": "medium",
                "message": "Wrong target."
            }]
        }))
        .unwrap();

        assert!(prepared.validate_output(&matching).is_ok());
        assert!(prepared.validate_output(&wrong).is_err());
    }
}
