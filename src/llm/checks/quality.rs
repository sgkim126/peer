use serde::Serialize;

use crate::cache::CacheKey;
use crate::extract::{CommitDiff, CommitFiles, ExtractError, Extractor};
use crate::git::CommitHash;
use crate::llm::context::ReviewContext;
use crate::llm::provider::ConversationTurn;

use super::tools;
use super::{CheckDefinition, PreparedCheck, PreparedCheckTarget, output_schema, system_prompt};

const SYSTEM_PROMPT: &str = r#"You are reviewing a single commit for general code quality.

Assess the changed code for:
1. Correctness issues and likely bugs.
2. Misuse of language or project idioms.
3. Unclear, unnecessarily complex, or fragile implementation.
4. Error handling, boundary conditions, and maintainability problems.
5. Missing tests when the change introduces behavior that requires coverage.

Focus on concrete issues introduced by the target commit. Security vulnerabilities or
attacker-controlled threat paths, commit structure or size, and message-to-diff alignment are
outside the scope of this check. Do not report unrelated pre-existing problems or purely
subjective style preferences.

Use the required commit data supplied by the user. Use tools when file context is needed. Every
finding must reference the target commit. Return no findings when no actionable issue is present."#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualityCheck {
    commit: CommitHash,
}

impl QualityCheck {
    pub async fn try_new(revision: &str, extractor: &Extractor) -> Result<Self, ExtractError> {
        Ok(Self {
            commit: extractor.resolve_commit(revision).await?,
        })
    }
}

impl CheckDefinition for QualityCheck {
    fn name(&self) -> &'static str {
        "quality"
    }

    fn cache_key(&self, provider: &str, model: &str, review_context: &ReviewContext) -> CacheKey {
        let params = QualityCheckCacheParams {
            commit: &self.commit,
            review_context,
        };

        CacheKey::from_params(self.name(), provider, model, &params)
            .expect("serializing quality check cache params cannot fail")
    }

    async fn prepare(
        &self,
        extractor: &Extractor,
        review_context: &ReviewContext,
    ) -> Result<PreparedCheck, ExtractError> {
        let diff = extractor.commit_diff(self.commit.as_ref()).await?;
        let files = extractor.commit_files(self.commit.as_ref()).await?;

        Ok(build_prepared_check(diff, files, review_context))
    }
}

#[derive(Debug, Serialize)]
struct QualityCheckCacheParams<'a> {
    commit: &'a CommitHash,
    review_context: &'a ReviewContext,
}

fn build_prepared_check(
    diff: CommitDiff,
    files: CommitFiles,
    review_context: &ReviewContext,
) -> PreparedCheck {
    let target = diff.hash.clone();
    let required_data = serde_json::json!({
        "target_commit": target,
        "changed_files": files.files,
        "diff": diff.diff,
    });
    let mut user_prompt = format!(
        "Review the following required commit data:\n{}",
        serde_json::to_string_pretty(&required_data)
            .expect("serializing quality check input cannot fail")
    );
    review_context.append_to_prompt(&mut user_prompt);

    PreparedCheck {
        conversation: vec![
            ConversationTurn::System(system_prompt(SYSTEM_PROMPT)),
            ConversationTurn::User(user_prompt),
        ],
        tools: vec![
            tools::get_commit_diff(),
            tools::get_changed_files(),
            tools::get_file_content(),
            tools::get_file_diff(),
            tools::list_tree(),
            tools::grep_search(),
            tools::request_user_info(),
        ],
        output_schema: output_schema(),
        target: PreparedCheckTarget::Commit(diff.hash),
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
            &ReviewContext::default(),
        )
    }

    #[test]
    fn name_is_quality() {
        let check = QualityCheck {
            commit: hash("abc1234"),
        };

        assert_eq!(check.name(), "quality");
    }

    #[test]
    fn prepared_conversation_contains_diff_and_changed_files() {
        let prepared = prepared_check();

        let ConversationTurn::System(system) = &prepared.conversation[0] else {
            panic!("expected system prompt");
        };
        assert!(system.contains("general code quality"));
        assert!(system.contains("outside the scope of this check"));
        assert!(system.contains("Tool use is optional"));
        assert!(system.contains("never invent\na tool name or arguments"));

        let ConversationTurn::User(user) = &prepared.conversation[1] else {
            panic!("expected required data");
        };
        assert!(user.contains("\"target_commit\": \"abc1234\""));
        assert!(user.contains("\"path\": \"src/lib.rs\""));
        assert!(user.contains("unwrap_input"));
        assert!(!user.contains("Review context:"));
    }

    #[test]
    fn prepared_conversation_contains_review_context_when_present() {
        let target = hash("abc1234");
        let prepared = build_prepared_check(
            CommitDiff {
                hash: target.clone(),
                diff: "diff --git a/src/lib.rs b/src/lib.rs\n+unwrap_input();".to_string(),
            },
            CommitFiles {
                hash: target,
                files: serde_json::from_value(serde_json::json!([])).unwrap(),
            },
            &ReviewContext {
                title: Some("Improve input handling".to_string()),
                body_summary: Some("Touches parser error paths.".to_string()),
                comments_summary: None,
            },
        );

        let ConversationTurn::User(user) = &prepared.conversation[1] else {
            panic!("expected required data");
        };
        assert!(user.contains("Review context:"));
        assert!(user.contains("Title:\nImprove input handling"));
        assert!(user.contains("Body summary:\nTouches parser error paths."));
    }

    #[test]
    fn prepared_check_uses_commit_target_and_common_inputs() {
        let prepared = prepared_check();

        assert_eq!(
            prepared.result_target(),
            CheckTarget::Commit(hash("abc1234"))
        );
        let tool_names = prepared
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            tool_names,
            [
                "get_commit_diff",
                "get_changed_files",
                "get_file_content",
                "get_file_diff",
                "list_tree",
                "grep_search",
                "request_user_info",
            ]
        );
        assert_eq!(
            prepared.output_schema["required"],
            serde_json::json!(["findings"])
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
            }]
        }))
        .unwrap();
        let wrong: CheckOutput = serde_json::from_value(serde_json::json!({
            "summary": "unchecked input",
            "findings": [{
                "commit": "def5678",
                "severity": "high",
                "message": "Wrong target."
            }]
        }))
        .unwrap();

        assert!(prepared.validate_output(&matching).is_ok());
        assert!(prepared.validate_output(&wrong).is_err());
    }
}
