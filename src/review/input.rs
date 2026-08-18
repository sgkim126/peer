use log::trace;
use serde::Serialize;

use crate::context::ReviewContext;
use crate::extract::{CommitFiles, CommitMessage, ExtractError, Extractor};
use crate::git::CommitHash;

use super::ReviewTarget;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ReviewCommitInput {
    pub hash: CommitHash,
    pub message: String,
    pub files: CommitFiles,
    pub diff: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ReviewInput {
    pub context: ReviewContext,
    pub base: Option<CommitHash>,
    pub head: CommitHash,
    pub commits: Vec<ReviewCommitInput>,
    pub cumulative_diff: String,
}

impl ReviewInput {
    pub async fn collect(
        target: &ReviewTarget,
        context: ReviewContext,
        extractor: &Extractor,
    ) -> Result<Self, ExtractError> {
        let (base, head, commits) = match target {
            ReviewTarget::Commit(commit) => (None, commit.clone(), vec![commit.clone()]),
            ReviewTarget::Range {
                from, to, commits, ..
            } => (Some(from.clone()), to.clone(), commits.clone()),
        };
        trace!("collecting review input: commits={}", commits.len());
        let mut inputs = Vec::with_capacity(commits.len());
        for commit in commits {
            let CommitMessage { hash, message } = extractor.commit_message(commit.as_ref()).await?;
            let files = extractor.commit_files(commit.as_ref()).await?;
            let diff = extractor.commit_diff(commit.as_ref()).await?.diff;
            inputs.push(ReviewCommitInput {
                hash,
                message,
                files,
                diff,
            });
        }
        let cumulative_diff = match &base {
            Some(base) => {
                extractor
                    .range_diff(base.as_ref(), head.as_ref())
                    .await?
                    .diff
            }
            None => inputs
                .first()
                .map(|commit| commit.diff.clone())
                .expect("single-commit review must contain one commit"),
        };

        trace!("review input collected: commits={}", inputs.len());
        Ok(Self {
            context,
            base,
            head,
            commits: inputs,
            cumulative_diff,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::git::run_git;

    async fn commit(directory: &std::path::Path, file: &str, message: &str) -> CommitHash {
        std::fs::write(directory.join(file), format!("{message}\n")).unwrap();
        run_git(&["add", file], directory).await.unwrap();
        run_git(&["commit", "--no-gpg-sign", "-m", message], directory)
            .await
            .unwrap();
        CommitHash::resolve("HEAD", directory).await.unwrap()
    }

    #[tokio::test]
    async fn collects_commits_oldest_first_and_the_cumulative_diff() {
        let directory = tempfile::tempdir().unwrap();
        run_git(&["init"], directory.path()).await.unwrap();
        run_git(
            &["config", "user.email", "test@example.com"],
            directory.path(),
        )
        .await
        .unwrap();
        run_git(&["config", "user.name", "Test"], directory.path())
            .await
            .unwrap();
        let base = commit(directory.path(), "base.txt", "base").await;
        let first = commit(directory.path(), "first.txt", "first").await;
        let second = commit(directory.path(), "second.txt", "second").await;
        let target = ReviewTarget::Range {
            from: base.clone(),
            to: second.clone(),
            commits: vec![first.clone(), second.clone()],
        };

        let input = ReviewInput::collect(
            &target,
            ReviewContext::default(),
            &Extractor::new(directory.path().to_path_buf()),
        )
        .await
        .unwrap();

        assert_eq!(input.base, Some(base));
        assert_eq!(input.head, second);
        assert_eq!(
            input
                .commits
                .iter()
                .map(|commit| commit.hash.clone())
                .collect::<Vec<_>>(),
            [first, input.head.clone()]
        );
        assert!(input.cumulative_diff.contains("first.txt"));
        assert!(input.cumulative_diff.contains("second.txt"));
    }
}
