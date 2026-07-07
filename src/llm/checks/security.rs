use serde::Serialize;

use crate::cache::CacheKey;
use crate::extract::{CommitDiff, CommitFiles, ExtractError, Extractor};
use crate::git::CommitHash;
use crate::llm::context::ReviewContext;
use crate::llm::provider::ConversationTurn;

use super::{CheckDefinition, PreparedCheck, PreparedCheckTarget, all_tools, output_schema};

const SYSTEM_PROMPT: &str = r#"You are performing an adversarial security review of a single commit.

Think like an attacker and assess whether the target commit introduces:
1. Authentication, authorization, or privilege-boundary failures.
2. Injection, unsafe parsing, path traversal, or command execution risks.
3. Secret exposure, insecure cryptography, or sensitive-data leakage.
4. Validation, deserialization, memory-safety, race, or denial-of-service vulnerabilities.
5. Unsafe defaults, trust-boundary violations, or security-relevant regressions.

Trace attacker-controlled inputs to sensitive operations. Distinguish exploitable issues from general code-quality concerns, and report only security findings introduced by the target commit. Use the required commit data supplied by the user. Use tools when surrounding file context is needed. Every finding must reference the target commit. Return no findings when no credible security issue is present."#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityCheck {
    commit: CommitHash,
}

impl SecurityCheck {
    pub async fn try_new(revision: &str, extractor: &Extractor) -> Result<Self, ExtractError> {
        Ok(Self {
            commit: extractor.resolve_commit(revision).await?,
        })
    }
}

impl CheckDefinition for SecurityCheck {
    fn name(&self) -> &'static str {
        "security"
    }

    fn cache_key(&self, provider: &str, model: &str, review_context: &ReviewContext) -> CacheKey {
        let params = SecurityCheckCacheParams {
            commit: &self.commit,
            review_context,
        };

        CacheKey::from_params(self.name(), provider, model, &params)
            .expect("serializing security check cache params cannot fail")
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
struct SecurityCheckCacheParams<'a> {
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
            .expect("serializing security check input cannot fail")
    );
    review_context.append_to_prompt(&mut user_prompt);

    PreparedCheck {
        conversation: vec![
            ConversationTurn::System(SYSTEM_PROMPT.to_string()),
            ConversationTurn::User(user_prompt),
        ],
        tools: all_tools(),
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
                diff: "diff --git a/src/auth.rs b/src/auth.rs\n+run(user_input);".to_string(),
            },
            CommitFiles {
                hash: target,
                files: serde_json::from_value(serde_json::json!([{
                    "path": "src/auth.rs",
                    "status": "modified",
                    "is_binary": false
                }]))
                .unwrap(),
            },
            &ReviewContext::default(),
        )
    }

    #[test]
    fn name_is_security() {
        let check = SecurityCheck {
            commit: hash("abc1234"),
        };

        assert_eq!(check.name(), "security");
    }

    #[test]
    fn prompt_uses_adversarial_security_perspective() {
        let prepared = prepared_check();

        let ConversationTurn::System(system) = &prepared.conversation[0] else {
            panic!("expected system prompt");
        };
        assert!(system.contains("adversarial security review"));
        assert!(system.contains("Think like an attacker"));
        assert!(system.contains("attacker-controlled inputs"));
    }

    #[test]
    fn prepared_conversation_contains_diff_and_changed_files() {
        let prepared = prepared_check();

        let ConversationTurn::User(user) = &prepared.conversation[1] else {
            panic!("expected required data");
        };
        assert!(user.contains("\"target_commit\": \"abc1234\""));
        assert!(user.contains("\"path\": \"src/auth.rs\""));
        assert!(user.contains("run(user_input)"));
        assert!(!user.contains("Review context:"));
    }

    #[test]
    fn prepared_conversation_contains_review_context_when_present() {
        let target = hash("abc1234");
        let prepared = build_prepared_check(
            CommitDiff {
                hash: target.clone(),
                diff: "diff --git a/src/auth.rs b/src/auth.rs\n+run(user_input);".to_string(),
            },
            CommitFiles {
                hash: target,
                files: serde_json::from_value(serde_json::json!([])).unwrap(),
            },
            &ReviewContext {
                title: Some("Harden command execution".to_string()),
                body_summary: None,
                comments_summary: Some("Reviewer mentioned command injection.".to_string()),
            },
        );

        let ConversationTurn::User(user) = &prepared.conversation[1] else {
            panic!("expected required data");
        };
        assert!(user.contains("Review context:"));
        assert!(user.contains("Title:\nHarden command execution"));
        assert!(user.contains("Comments summary:\nReviewer mentioned command injection."));
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
            "summary": "command injection",
            "findings": [{
                "commit": "abc1234",
                "severity": "critical",
                "message": "Untrusted input reaches command execution."
            }],
            "confidence": 0.9
        }))
        .unwrap();
        let wrong: CheckOutput = serde_json::from_value(serde_json::json!({
            "summary": "command injection",
            "findings": [{
                "commit": "def5678",
                "severity": "critical",
                "message": "Wrong target."
            }],
            "confidence": 0.9
        }))
        .unwrap();

        assert!(prepared.validate_output(&matching).is_ok());
        assert!(prepared.validate_output(&wrong).is_err());
    }
}
