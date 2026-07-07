use serde::Serialize;

use crate::cache::CacheKey;
use crate::extract::{CommitDiff, CommitFiles, CommitMessage, ExtractError, Extractor};
use crate::git::CommitHash;
use crate::llm::context::ReviewContext;
use crate::llm::provider::ConversationTurn;

use super::{CheckDefinition, PreparedCheck, PreparedCheckTarget, all_tools, output_schema};

const SYSTEM_PROMPT: &str = r#"You are reviewing a single commit for size and completeness.

Assess whether the commit:
1. Has one coherent purpose rather than mixing unrelated changes.
2. Combines refactoring and behavior changes in a way that should be split.
3. Is too large to review or revert safely as one unit.
4. Appears incomplete or requires missing companion changes.

Use the required commit data supplied by the user. Use tools only when additional context is needed. Every finding must reference the target commit. Return no findings when the commit is appropriately scoped and complete."#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SizeCheck {
    commit: CommitHash,
}

impl SizeCheck {
    pub async fn try_new(revision: &str, extractor: &Extractor) -> Result<Self, ExtractError> {
        Ok(Self {
            commit: extractor.resolve_commit(revision).await?,
        })
    }
}

impl CheckDefinition for SizeCheck {
    fn name(&self) -> &'static str {
        "size"
    }

    fn cache_key(&self, provider: &str, model: &str, review_context: &ReviewContext) -> CacheKey {
        let params = SizeCheckCacheParams {
            commit: &self.commit,
            review_context,
        };

        CacheKey::from_params(self.name(), provider, model, &params)
            .expect("serializing size check cache params cannot fail")
    }

    async fn prepare(
        &self,
        extractor: &Extractor,
        review_context: &ReviewContext,
    ) -> Result<PreparedCheck, ExtractError> {
        let message = extractor.commit_message(self.commit.as_ref()).await?;
        let diff = extractor.commit_diff(self.commit.as_ref()).await?;
        let files = extractor.commit_files(self.commit.as_ref()).await?;

        Ok(build_prepared_check(message, diff, files, review_context))
    }
}

#[derive(Debug, Serialize)]
struct SizeCheckCacheParams<'a> {
    commit: &'a CommitHash,
    review_context: &'a ReviewContext,
}

fn build_prepared_check(
    message: CommitMessage,
    diff: CommitDiff,
    files: CommitFiles,
    review_context: &ReviewContext,
) -> PreparedCheck {
    let target = message.hash.clone();
    let required_data = serde_json::json!({
        "target_commit": target,
        "commit_message": message.message,
        "changed_files": files.files,
        "diff": diff.diff,
    });
    let mut user_prompt = format!(
        "Review the following required commit data:\n{}",
        serde_json::to_string_pretty(&required_data)
            .expect("serializing size check input cannot fail")
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
                message: "Add size check".to_string(),
            },
            CommitDiff {
                hash: target.clone(),
                diff: "diff --git a/src/a.rs b/src/a.rs\n+new line".to_string(),
            },
            CommitFiles {
                hash: target,
                files: serde_json::from_value(serde_json::json!([{
                    "path": "src/a.rs",
                    "status": "modified",
                    "is_binary": false
                }]))
                .unwrap(),
            },
            &ReviewContext::default(),
        )
    }

    #[test]
    fn name_is_size() {
        let check = SizeCheck {
            commit: hash("abc1234"),
        };

        assert_eq!(check.name(), "size");
    }

    #[test]
    fn prepared_conversation_contains_required_commit_data() {
        let prepared = prepared_check();

        let ConversationTurn::System(system) = &prepared.conversation[0] else {
            panic!("expected system prompt");
        };
        assert!(system.contains("size and completeness"));

        let ConversationTurn::User(user) = &prepared.conversation[1] else {
            panic!("expected required data");
        };
        assert!(user.contains("\"target_commit\": \"abc1234\""));
        assert!(user.contains("\"commit_message\": \"Add size check\""));
        assert!(user.contains("\"path\": \"src/a.rs\""));
        assert!(user.contains("+new line"));
        assert!(!user.contains("Review context:"));
    }

    #[test]
    fn prepared_conversation_contains_review_context_when_present() {
        let target = hash("abc1234");
        let prepared = build_prepared_check(
            CommitMessage {
                hash: target.clone(),
                message: "Add size check".to_string(),
            },
            CommitDiff {
                hash: target.clone(),
                diff: "diff --git a/src/a.rs b/src/a.rs\n+new line".to_string(),
            },
            CommitFiles {
                hash: target,
                files: serde_json::from_value(serde_json::json!([])).unwrap(),
            },
            &ReviewContext {
                title: Some("Split large change".to_string()),
                body_summary: None,
                comments_summary: Some("Reviewer asked whether this should be split.".to_string()),
            },
        );

        let ConversationTurn::User(user) = &prepared.conversation[1] else {
            panic!("expected required data");
        };
        assert!(user.contains("Review context:"));
        assert!(user.contains("Title:\nSplit large change"));
        assert!(user.contains("Comments summary:\nReviewer asked whether this should be split."));
    }

    #[test]
    fn prepared_check_exposes_all_follow_up_tools() {
        let prepared = prepared_check();
        let names = prepared
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            [
                "get_commit_message",
                "get_commit_diff",
                "get_changed_files",
                "get_commits_in_range",
                "get_file_content",
            ]
        );
    }

    #[test]
    fn follow_up_tools_accept_revisions() {
        let prepared = prepared_check();

        for tool_name in ["get_commit_message", "get_commit_diff", "get_changed_files"] {
            let tool = prepared
                .tools
                .iter()
                .find(|tool| tool.name == tool_name)
                .unwrap();
            assert!(tool.parameters["properties"].get("revision").is_some());
            assert_eq!(tool.parameters["required"], serde_json::json!(["revision"]));
        }

        let file_content = prepared
            .tools
            .iter()
            .find(|tool| tool.name == "get_file_content")
            .unwrap();
        assert!(
            file_content.parameters["properties"]
                .get("revision")
                .is_some()
        );
        assert_eq!(
            file_content.parameters["required"],
            serde_json::json!(["path", "revision"])
        );
    }

    #[test]
    fn prepared_check_uses_commit_target_and_common_output_schema() {
        let prepared = prepared_check();

        assert_eq!(
            prepared.result_target(),
            CheckTarget::Commit(hash("abc1234"))
        );
        assert_eq!(
            prepared.output_schema["required"],
            serde_json::json!(["summary", "findings", "confidence"])
        );
    }

    #[test]
    fn prepared_check_validates_finding_commit() {
        let prepared = prepared_check();
        let matching: CheckOutput = serde_json::from_value(serde_json::json!({
            "summary": "too large",
            "findings": [{
                "commit": "abc1234",
                "severity": "medium",
                "message": "mixes unrelated changes"
            }],
            "confidence": 0.9
        }))
        .unwrap();
        let wrong: CheckOutput = serde_json::from_value(serde_json::json!({
            "summary": "too large",
            "findings": [{
                "commit": "def5678",
                "severity": "medium",
                "message": "mixes unrelated changes"
            }],
            "confidence": 0.9
        }))
        .unwrap();

        assert!(prepared.validate_output(&matching).is_ok());
        assert!(prepared.validate_output(&wrong).is_err());
    }
}
