use crate::extract::{CommitDiff, CommitFiles, CommitMessage, ExtractError, Extractor};
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
    revision: String,
}

impl SizeCheck {
    pub fn new(revision: String) -> Self {
        Self { revision }
    }
}

impl CheckDefinition for SizeCheck {
    fn name(&self) -> &'static str {
        "size"
    }

    async fn prepare(&self, extractor: &Extractor) -> Result<PreparedCheck, ExtractError> {
        let message = extractor.commit_message(&self.revision).await?;
        let target = message.hash.clone();
        let diff = extractor.commit_diff(target.as_ref()).await?;
        let files = extractor.commit_files(target.as_ref()).await?;

        Ok(build_prepared_check(message, diff, files))
    }
}

fn build_prepared_check(
    message: CommitMessage,
    diff: CommitDiff,
    files: CommitFiles,
) -> PreparedCheck {
    let target = message.hash.clone();
    let required_data = serde_json::json!({
        "target_commit": target,
        "commit_message": message.message,
        "changed_files": files.files,
        "diff": diff.diff,
    });

    PreparedCheck {
        conversation: vec![
            ConversationTurn::System(SYSTEM_PROMPT.to_string()),
            ConversationTurn::User(format!(
                "Review the following required commit data:\n{}",
                serde_json::to_string_pretty(&required_data)
                    .expect("serializing size check input cannot fail")
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
        )
    }

    #[test]
    fn name_is_size() {
        assert_eq!(SizeCheck::new("HEAD".to_string()).name(), "size");
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
