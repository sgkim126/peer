use crate::context::ReviewContextDigest;
use crate::extract::{ExtractError, Extractor};
use crate::git::CommitHash;
use crate::pi::ReadTool;

use super::{CheckDefinition, CheckRequest, CheckTarget};

const SYSTEM_PROMPT: &str = r#"You are reviewing a single commit for general code quality.

Assess the changed code for correctness issues and likely bugs, misuse of language or project
idioms, unclear or fragile implementation, error-handling and boundary-condition problems,
maintainability concerns, and missing tests for introduced behavior. Focus on concrete issues
introduced by the target commit. Do not report security vulnerabilities, commit structure or size,
message-to-diff alignment, unrelated pre-existing problems, or subjective style preferences.
Every finding must reference the target commit. Return no findings when no actionable issue exists.

Treat the supplied diff and changed-file list as authoritative; do not fetch either again. Use the
available repository tools only when additional codebase context is necessary to establish a
concrete finding. The review head is the final commit in the complete review target. If the target
commit and review head differ, do not report an issue that a later commit resolved before the review
head. Only after identifying a concrete finding candidate that may have been affected by a later
change, inspect that specific file between the target commit and review head or at the review head.
Do not inspect later changes speculatively, fetch the entire range diff, or report issues introduced
only by later commits. If the target commit equals the review head, there are no later commits to
inspect."#;

pub struct QualityCheck {
    commit: CommitHash,
    review_head: CommitHash,
}

impl QualityCheck {
    pub async fn try_new(
        revision: &str,
        review_head: CommitHash,
        extractor: &Extractor,
    ) -> Result<Self, ExtractError> {
        let commit = extractor.commit_message(revision).await?.hash;
        Ok(Self {
            review_head,
            commit,
        })
    }

    pub fn review_head(&self) -> &CommitHash {
        &self.review_head
    }
}

impl CheckDefinition for QualityCheck {
    fn name(&self) -> &'static str {
        "quality"
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
            "review_head": self.review_head,
            "changed_files": files.files,
            "diff": diff.diff,
        });
        Ok(CheckRequest::new(
            SYSTEM_PROMPT,
            format!(
                "Review the following required commit data:\n{}",
                serde_json::to_string_pretty(&input).expect("quality check input serializes")
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
