use crate::context::ReviewContextDigest;
use crate::extract::{ExtractError, Extractor};
use crate::git::CommitHash;
use crate::pi::ReadTool;

use super::{CheckDefinition, CheckRequest, CheckTarget};

const SYSTEM_PROMPT: &str = r#"You are reviewing a single commit for alignment between its stated intent and its actual changes.

Assess whether the commit message accurately describes the behavior and scope of the diff, whether
the diff contains unrelated or undocumented changes, whether the message claims work absent from
the diff, and whether important user-visible or operational effects are omitted. Do not assess
correctness, style, tests, security, or commit splitting. Every finding must reference the target
commit. Return no findings when the message and diff are aligned.

Treat the supplied commit message and diff as authoritative; do not fetch either again. Use the
changed-file list or file content only when additional repository context is necessary to compare
the supplied message and diff."#;

pub struct IntentCheck {
    commit: CommitHash,
}

impl IntentCheck {
    pub async fn try_new(revision: &str, extractor: &Extractor) -> Result<Self, ExtractError> {
        Ok(Self {
            commit: extractor.commit_message(revision).await?.hash,
        })
    }
}

impl CheckDefinition for IntentCheck {
    fn name(&self) -> &'static str {
        "intent"
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
        let message = extractor.commit_message(self.commit.as_ref()).await?;
        let diff = extractor.commit_diff(self.commit.as_ref()).await?;
        let input = serde_json::json!({
            "target_commit": self.commit,
            "commit_message": message.message,
            "diff": diff.diff,
        });
        Ok(CheckRequest::new(
            SYSTEM_PROMPT,
            format!(
                "Review the following required commit data:\n{}",
                serde_json::to_string_pretty(&input).expect("intent check input serializes")
            ),
            vec![ReadTool::GetChangedFiles, ReadTool::GetFileContent],
            review_context,
        ))
    }
}
