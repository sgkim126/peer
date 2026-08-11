use crate::context::ReviewContextDigest;
use crate::extract::{ExtractError, Extractor};
use crate::git::CommitHash;
use crate::pi::ReadTool;

use super::{CheckDefinition, CheckRequest, CheckTarget};

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

    async fn request(
        &self,
        extractor: &Extractor,
        review_context: &ReviewContextDigest,
    ) -> Result<CheckRequest, ExtractError> {
        let diff = extractor.commit_diff(self.commit.as_ref()).await?;
        let files = extractor.commit_files(self.commit.as_ref()).await?;
        let input = serde_json::json!({
            "target_commit": self.commit,
            "changed_files": files.files,
            "diff": diff.diff,
        });
        Ok(CheckRequest::new(
            SYSTEM_PROMPT,
            format!(
                "Review the following required commit data:\n{}",
                serde_json::to_string_pretty(&input).expect("security check input serializes")
            ),
            vec![
                ReadTool::GetFileContent,
                ReadTool::GetFileDiff,
                ReadTool::ListTree,
                ReadTool::Grep,
            ],
            review_context,
        ))
    }
}
