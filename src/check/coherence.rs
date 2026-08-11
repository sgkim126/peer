use crate::context::ReviewContextDigest;
use crate::extract::{CommitList, ExtractError, Extractor};
use crate::git::CommitHash;
use crate::llm::CheckTarget;
use crate::pi::ReadTool;

use super::{CheckDefinition, CheckRequest};

const SYSTEM_PROMPT: &str = r#"You are reviewing a commit series for coherence.

Assess commit ordering, dependencies, narrative flow, unnecessary fixups or reversions, confusing
splits of one logical change, mixed responsibilities, and whether the message sequence clearly
communicates work progression. Do not report individual correctness, style, tests, security, or
message-to-diff alignment. Every finding must reference the commit responsible for the
series-level issue. Return no findings when the series forms a clear progression.

Treat the supplied ordered commit messages as authoritative; do not fetch them again. Use diff or
changed-file lookup only when additional evidence is necessary to establish a relationship between
commits."#;

pub struct CoherenceCheck {
    commits: CommitList,
}

impl CoherenceCheck {
    pub async fn try_new(range: &str, extractor: &Extractor) -> Result<Self, ExtractError> {
        Ok(Self {
            commits: extractor.commit_list(range).await?,
        })
    }
}

impl CheckDefinition for CoherenceCheck {
    fn name(&self) -> &'static str {
        "coherence"
    }

    fn target(&self) -> CheckTarget {
        CheckTarget::Range {
            from: self.commits.from.clone(),
            to: self.commits.to.clone(),
        }
    }

    fn expected_commits(&self) -> &[CommitHash] {
        &self.commits.commits
    }

    async fn request(
        &self,
        extractor: &Extractor,
        review_context: &ReviewContextDigest,
    ) -> Result<CheckRequest, ExtractError> {
        let mut entries = Vec::with_capacity(self.commits.commits.len());
        for (index, commit) in self.commits.commits.iter().enumerate() {
            let message = extractor.commit_message(commit.as_ref()).await?;
            entries.push(format!(
                "{}. {}\n{}",
                index + 1,
                message.hash,
                indent(&message.message)
            ));
        }
        Ok(CheckRequest::new(
            SYSTEM_PROMPT,
            format!(
                "Review range {}.\n\nCommits (oldest to newest):\n{}",
                self.commits.range,
                entries.join("\n\n")
            ),
            vec![ReadTool::GetCommitDiff, ReadTool::GetChangedFiles],
            review_context,
        ))
    }
}

fn indent(value: &str) -> String {
    value
        .lines()
        .map(|line| format!("   {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}
