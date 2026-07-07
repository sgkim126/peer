use crate::extract::{CommitDiff, CommitFiles, ExtractError, Extractor};
use crate::llm::provider::ConversationTurn;

use super::{CheckDefinition, PreparedCheck, PreparedCheckTarget, all_tools, output_schema};

const SYSTEM_PROMPT: &str = r#"You are reviewing a single commit for general code quality.

Assess the changed code for:
1. Correctness issues and likely bugs.
2. Misuse of language or project idioms.
3. Unclear, unnecessarily complex, or fragile implementation.
4. Error handling, boundary conditions, and maintainability problems.
5. Missing tests when the change introduces behavior that requires coverage.

Focus on concrete issues introduced by the target commit. Do not report unrelated pre-existing problems or purely subjective style preferences. Use the required commit data supplied by the user. Use tools when file context is needed. Every finding must reference the target commit. Return no findings when no actionable issue is present."#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualityCheck {
    revision: String,
}

impl QualityCheck {
    pub fn new(revision: String) -> Self {
        Self { revision }
    }
}

impl CheckDefinition for QualityCheck {
    fn name(&self) -> &'static str {
        "quality"
    }

    async fn prepare(
        &self,
        extractor: &Extractor,
        _review_context: &crate::llm::context::ReviewContext,
    ) -> Result<PreparedCheck, ExtractError> {
        let diff = extractor.commit_diff(&self.revision).await?;
        let files = extractor.commit_files(diff.hash.as_ref()).await?;

        Ok(build_prepared_check(diff, files))
    }
}

fn build_prepared_check(diff: CommitDiff, files: CommitFiles) -> PreparedCheck {
    let target = diff.hash.clone();
    let required_data = serde_json::json!({
        "target_commit": target,
        "changed_files": files.files,
        "diff": diff.diff,
    });

    PreparedCheck {
        conversation: vec![
            ConversationTurn::System(SYSTEM_PROMPT.to_string()),
            ConversationTurn::User(format!(
                "Review the following required commit data:\n{}",
                serde_json::to_string_pretty(&required_data)
                    .expect("serializing quality check input cannot fail")
            )),
        ],
        tools: all_tools(),
        output_schema: output_schema(),
        target: PreparedCheckTarget::Commit(diff.hash),
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
            CommitDiff {
                hash: target.clone(),
                diff: "diff --git a/src/lib.rs b/src/lib.rs\n+unwrap_input();".to_string(),
            },
            CommitFiles {
                hash: target,
                files: serde_json::from_value(serde_json::json!([{
                    "path": "src/lib.rs",
                    "status": "modified",
                    "is_binary": false
                }]))
                .unwrap(),
            },
        )
    }

    #[test]
    fn name_is_quality() {
        assert_eq!(QualityCheck::new("HEAD".to_string()).name(), "quality");
    }

    #[test]
    fn prepared_conversation_contains_diff_and_changed_files() {
        let prepared = prepared_check();

        let ConversationTurn::System(system) = &prepared.conversation[0] else {
            panic!("expected system prompt");
        };
        assert!(system.contains("general code quality"));

        let ConversationTurn::User(user) = &prepared.conversation[1] else {
            panic!("expected required data");
        };
        assert!(user.contains("\"target_commit\": \"abc1234\""));
        assert!(user.contains("\"path\": \"src/lib.rs\""));
        assert!(user.contains("unwrap_input"));
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
            "summary": "unchecked input",
            "findings": [{
                "commit": "abc1234",
                "severity": "high",
                "message": "Untrusted input is unwrapped."
            }],
            "confidence": 0.9
        }))
        .unwrap();
        let wrong: CheckOutput = serde_json::from_value(serde_json::json!({
            "summary": "unchecked input",
            "findings": [{
                "commit": "def5678",
                "severity": "high",
                "message": "Wrong target."
            }],
            "confidence": 0.9
        }))
        .unwrap();

        assert!(prepared.validate_output(&matching).is_ok());
        assert!(prepared.validate_output(&wrong).is_err());
    }
}
