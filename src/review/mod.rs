use std::fmt;
use std::path::{Path, PathBuf};

use crate::cli::CheckCommand;
use crate::config::Config;
use crate::console::Console;
use crate::git::{CommitHash, GitError, run_git};
use crate::llm::checks::{self, CheckCommandError};
use crate::llm::context::ReviewContext;
use crate::llm::result::CheckOutcome;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewTarget {
    Commit(CommitHash),
    Range {
        revision: String,
        commits: Vec<CommitHash>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewPlan {
    pub checks: Vec<ReviewCheck>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ReviewResult {
    pub outcomes: Vec<CheckOutcome>,

    #[serde(skip, default)]
    pub errors: Vec<ReviewCheckError>,
}

#[derive(Debug)]
pub struct ReviewCheckError {
    pub check: ReviewCheck,
    pub error: CheckCommandError,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewCheck {
    Size { revision: String },
    Intent { revision: String },
    Quality { revision: String },
    Security { revision: String },
    Coherence { range: String },
}

impl From<ReviewCheck> for CheckCommand {
    fn from(check: ReviewCheck) -> Self {
        match check {
            ReviewCheck::Size { revision } => Self::Size { revision },
            ReviewCheck::Intent { revision } => Self::Intent { revision },
            ReviewCheck::Quality { revision } => Self::Quality { revision },
            ReviewCheck::Security { revision } => Self::Security { revision },
            ReviewCheck::Coherence { range } => Self::Coherence { range },
        }
    }
}

impl fmt::Display for ReviewCheck {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Size { revision } => write!(f, "size {revision}"),
            Self::Intent { revision } => write!(f, "intent {revision}"),
            Self::Quality { revision } => write!(f, "quality {revision}"),
            Self::Security { revision } => write!(f, "security {revision}"),
            Self::Coherence { range } => write!(f, "coherence {range}"),
        }
    }
}

impl fmt::Display for ReviewCheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} failed: {}", self.check, self.error)
    }
}

impl std::error::Error for ReviewCheckError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

pub fn plan_checks(target: &ReviewTarget) -> ReviewPlan {
    let mut checks = Vec::new();

    match target {
        ReviewTarget::Commit(commit) => {
            append_commit_checks(&mut checks, commit);
        }
        ReviewTarget::Range { revision, commits } => {
            for commit in commits {
                append_commit_checks(&mut checks, commit);
            }
            checks.push(ReviewCheck::Coherence {
                range: revision.clone(),
            });
        }
    }

    ReviewPlan { checks }
}

fn append_commit_checks(checks: &mut Vec<ReviewCheck>, commit: &CommitHash) {
    let revision = commit.to_string();
    checks.push(ReviewCheck::Size {
        revision: revision.clone(),
    });
    checks.push(ReviewCheck::Intent {
        revision: revision.clone(),
    });
    checks.push(ReviewCheck::Quality {
        revision: revision.clone(),
    });
    checks.push(ReviewCheck::Security { revision });
}

pub async fn run(
    plan: ReviewPlan,
    console: Console,
    config: &Config,
    project_root: PathBuf,
    review_context: &ReviewContext,
) -> ReviewResult {
    let mut outcomes = Vec::with_capacity(plan.checks.len());
    let mut errors = Vec::new();

    for check in plan.checks {
        let command = CheckCommand::from(check.clone());
        match checks::handler(
            console,
            command,
            config,
            project_root.clone(),
            review_context,
        )
        .await
        {
            Ok(outcome) => outcomes.push(outcome),
            Err(error) => errors.push(ReviewCheckError { check, error }),
        }
    }

    ReviewResult { outcomes, errors }
}

pub async fn resolve_target(
    target: &str,
    project_root: &Path,
    console: Console,
) -> Result<ReviewTarget, ReviewTargetError> {
    if !target.contains("..") {
        let commit = CommitHash::resolve(target, project_root, console).await?;

        return Ok(ReviewTarget::Commit(commit));
    }

    if target.contains("...") {
        return Err(ReviewTargetError::InvalidRange(target.to_string()));
    }
    let Some((from, to)) = target.split_once("..") else {
        return Err(ReviewTargetError::InvalidRange(target.to_string()));
    };
    if from.is_empty() || to.is_empty() {
        return Err(ReviewTargetError::InvalidRange(target.to_string()));
    }

    let output = run_git(&["rev-list", "--reverse", target], project_root, console).await?;
    let commits = output
        .lines()
        .filter(|line| !line.is_empty())
        .map(CommitHash::new)
        .collect::<Result<Vec<_>, _>>()?;
    if commits.is_empty() {
        return Err(ReviewTargetError::EmptyRange(target.to_string()));
    }

    Ok(ReviewTarget::Range {
        revision: target.to_string(),
        commits,
    })
}

pub async fn validate_target(
    target: &ReviewTarget,
    max_commits: u32,
    project_root: &Path,
    console: Console,
) -> Result<(), ReviewTargetError> {
    let commits = match target {
        ReviewTarget::Commit(commit) => std::slice::from_ref(commit),
        ReviewTarget::Range { commits, .. } => commits,
    };

    if commits.len() > max_commits as usize {
        return Err(ReviewTargetError::TooManyCommits {
            actual: commits.len(),
            maximum: max_commits,
        });
    }

    for commit in commits {
        let output = run_git(
            &["rev-list", "--parents", "-n", "1", commit.as_ref()],
            project_root,
            console,
        )
        .await?;
        if output.split_whitespace().count() > 2 {
            return Err(ReviewTargetError::MergeCommit(commit.clone()));
        }
    }

    Ok(())
}

#[derive(Debug)]
pub enum ReviewTargetError {
    Git(GitError),
    InvalidRange(String),
    EmptyRange(String),
    TooManyCommits { actual: usize, maximum: u32 },
    MergeCommit(CommitHash),
}

impl fmt::Display for ReviewTargetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Git(error) => error.fmt(f),
            Self::InvalidRange(range) => write!(f, "{range} is not a two-dots range"),
            Self::EmptyRange(range) => write!(f, "{range} contains no commits"),
            Self::TooManyCommits { actual, maximum } => {
                write!(
                    f,
                    "review target contains {actual} commits, exceeding the maximum of {maximum}"
                )
            }
            Self::MergeCommit(commit) => {
                write!(f, "review target contains merge commit {commit}")
            }
        }
    }
}

impl std::error::Error for ReviewTargetError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Git(error) => Some(error),
            Self::InvalidRange(_) => None,
            Self::EmptyRange(_) => None,
            Self::TooManyCommits { .. } => None,
            Self::MergeCommit(_) => None,
        }
    }
}

impl From<GitError> for ReviewTargetError {
    fn from(error: GitError) -> Self {
        Self::Git(error)
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::git::run_git;
    use crate::llm::confidence::Confidence;
    use crate::llm::result::{CheckResult, CheckTarget, CheckUsage};

    struct Repo {
        _tmp: TempDir,
        path: std::path::PathBuf,
    }

    impl Repo {
        async fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().to_path_buf();
            let console = Console::default();
            run_git(&["init"], &path, console).await.unwrap();
            run_git(
                &["config", "user.email", "test@example.com"],
                &path,
                console,
            )
            .await
            .unwrap();
            run_git(&["config", "user.name", "Test"], &path, console)
                .await
                .unwrap();

            Self { _tmp: tmp, path }
        }

        async fn commit(&self, file: &str, message: &str) -> CommitHash {
            let console = Console::default();
            std::fs::write(self.path.join(file), message).unwrap();
            run_git(&["add", file], &self.path, console).await.unwrap();
            run_git(
                &["commit", "--no-gpg-sign", "-m", message],
                &self.path,
                console,
            )
            .await
            .unwrap();
            let hash = run_git(&["rev-parse", "HEAD"], &self.path, console)
                .await
                .unwrap();

            CommitHash::new(hash.trim()).unwrap()
        }
    }

    fn check_result() -> CheckResult {
        CheckResult {
            check: "size".to_string(),
            target: CheckTarget::Commit(CommitHash::new("abc1234").unwrap()),
            summary: "summary".to_string(),
            findings: Vec::new(),
            confidence: Confidence::try_from(0.9).unwrap(),
            iterations: 1,
            is_exhausted: false,
            exhaustion_reason: None,
            usage: CheckUsage {
                input_tokens: 10,
                output_tokens: 20,
                cost_usd: 0.001,
                model: "test-model".to_string(),
            },
        }
    }

    #[tokio::test]
    async fn resolves_single_revision_to_tip_commit() {
        let repo = Repo::new().await;
        let expected = repo.commit("a.txt", "first").await;

        let target = resolve_target("HEAD", &repo.path, Console::default())
            .await
            .unwrap();

        assert_eq!(target, ReviewTarget::Commit(expected));
    }

    #[tokio::test]
    async fn resolves_range_oldest_to_newest() {
        let repo = Repo::new().await;
        let base = repo.commit("a.txt", "first").await;
        let second = repo.commit("b.txt", "second").await;
        let third = repo.commit("c.txt", "third").await;
        let revision = format!("{base}..HEAD");

        let target = resolve_target(&revision, &repo.path, Console::default())
            .await
            .unwrap();

        assert_eq!(
            target,
            ReviewTarget::Range {
                revision,
                commits: vec![second, third],
            }
        );
    }

    #[tokio::test]
    async fn rejects_three_dot_range() {
        let repo = Repo::new().await;
        repo.commit("a.txt", "first").await;
        let revision = "HEAD...HEAD";

        let error = resolve_target(revision, &repo.path, Console::default())
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            ReviewTargetError::InvalidRange(value) if value == revision
        ));
    }

    #[tokio::test]
    async fn rejects_empty_range() {
        let repo = Repo::new().await;
        repo.commit("a.txt", "first").await;
        let revision = "HEAD..HEAD";

        let error = resolve_target(revision, &repo.path, Console::default())
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            ReviewTargetError::EmptyRange(value) if value == revision
        ));
    }

    #[tokio::test]
    async fn rejects_invalid_single_revision() {
        let repo = Repo::new().await;

        let error = resolve_target("missing", &repo.path, Console::default())
            .await
            .unwrap_err();

        assert!(matches!(error, ReviewTargetError::Git(_)));
    }

    #[tokio::test]
    async fn rejects_target_exceeding_max_commits() {
        let repo = Repo::new().await;
        let first = repo.commit("a.txt", "first").await;
        let second = repo.commit("b.txt", "second").await;
        let target = ReviewTarget::Range {
            revision: format!("{first}..{second}"),
            commits: vec![first, second],
        };

        let error = validate_target(&target, 1, &repo.path, Console::default())
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            ReviewTargetError::TooManyCommits {
                actual: 2,
                maximum: 1
            }
        ));
    }

    #[tokio::test]
    async fn accepts_target_within_max_commits() {
        let repo = Repo::new().await;
        let commit = repo.commit("a.txt", "first").await;
        let target = ReviewTarget::Commit(commit);

        validate_target(&target, 1, &repo.path, Console::default())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn rejects_single_merge_commit() {
        let repo = Repo::new().await;
        repo.commit("base.txt", "base").await;
        let main_branch = run_git(
            &["branch", "--show-current"],
            &repo.path,
            Console::default(),
        )
        .await
        .unwrap();
        run_git(
            &["checkout", "-b", "feature"],
            &repo.path,
            Console::default(),
        )
        .await
        .unwrap();
        repo.commit("feature.txt", "feature").await;
        run_git(
            &["checkout", main_branch.trim()],
            &repo.path,
            Console::default(),
        )
        .await
        .unwrap();
        repo.commit("main.txt", "main").await;
        run_git(
            &["merge", "--no-ff", "--no-edit", "feature"],
            &repo.path,
            Console::default(),
        )
        .await
        .unwrap();
        let merge = CommitHash::resolve("HEAD", &repo.path, Console::default())
            .await
            .unwrap();
        let target = ReviewTarget::Commit(merge.clone());

        let error = validate_target(&target, 10, &repo.path, Console::default())
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            ReviewTargetError::MergeCommit(commit) if commit == merge
        ));
    }

    #[test]
    fn plans_single_commit_checks() {
        let commit_hash = "abc1234";
        let commit = CommitHash::new(commit_hash).unwrap();
        let target = ReviewTarget::Commit(commit);

        let plan = plan_checks(&target);

        assert_eq!(
            plan,
            ReviewPlan {
                checks: vec![
                    ReviewCheck::Size {
                        revision: commit_hash.to_string()
                    },
                    ReviewCheck::Intent {
                        revision: commit_hash.to_string()
                    },
                    ReviewCheck::Quality {
                        revision: commit_hash.to_string()
                    },
                    ReviewCheck::Security {
                        revision: commit_hash.to_string()
                    },
                ]
            }
        );
    }

    #[test]
    fn plans_range_checks() {
        let first_commit_hash = "abc1234";
        let second_commit_hash = "def5678";
        let first = CommitHash::new(first_commit_hash).unwrap();
        let second = CommitHash::new(second_commit_hash).unwrap();
        let revision = "main..HEAD".to_string();
        let target = ReviewTarget::Range {
            revision: revision.clone(),
            commits: vec![first, second],
        };

        let plan = plan_checks(&target);

        assert_eq!(
            plan,
            ReviewPlan {
                checks: vec![
                    ReviewCheck::Size {
                        revision: first_commit_hash.to_string()
                    },
                    ReviewCheck::Intent {
                        revision: first_commit_hash.to_string()
                    },
                    ReviewCheck::Quality {
                        revision: first_commit_hash.to_string()
                    },
                    ReviewCheck::Security {
                        revision: first_commit_hash.to_string()
                    },
                    ReviewCheck::Size {
                        revision: second_commit_hash.to_string()
                    },
                    ReviewCheck::Intent {
                        revision: second_commit_hash.to_string()
                    },
                    ReviewCheck::Quality {
                        revision: second_commit_hash.to_string()
                    },
                    ReviewCheck::Security {
                        revision: second_commit_hash.to_string()
                    },
                    ReviewCheck::Coherence { range: revision },
                ]
            }
        );
    }

    #[test]
    fn converts_review_check_to_check_command() {
        assert_eq!(
            CheckCommand::from(ReviewCheck::Size {
                revision: "abc1234".to_string()
            }),
            CheckCommand::Size {
                revision: "abc1234".to_string()
            }
        );
        assert_eq!(
            CheckCommand::from(ReviewCheck::Intent {
                revision: "abc1234".to_string()
            }),
            CheckCommand::Intent {
                revision: "abc1234".to_string()
            }
        );
        assert_eq!(
            CheckCommand::from(ReviewCheck::Quality {
                revision: "abc1234".to_string()
            }),
            CheckCommand::Quality {
                revision: "abc1234".to_string()
            }
        );
        assert_eq!(
            CheckCommand::from(ReviewCheck::Security {
                revision: "abc1234".to_string()
            }),
            CheckCommand::Security {
                revision: "abc1234".to_string()
            }
        );
        assert_eq!(
            CheckCommand::from(ReviewCheck::Coherence {
                range: "main..HEAD".to_string()
            }),
            CheckCommand::Coherence {
                range: "main..HEAD".to_string()
            }
        );
    }

    #[test]
    fn creates_review_result_from_check_results() {
        let check = check_result();

        let result = ReviewResult {
            outcomes: vec![CheckOutcome::success(check.clone())],
            errors: Default::default(),
        };

        assert_eq!(result.outcomes, vec![CheckOutcome::success(check)]);
    }

    #[test]
    fn serializes_review_result() {
        let result = ReviewResult {
            outcomes: vec![CheckOutcome::success(check_result())],
            errors: Default::default(),
        };

        let value = serde_json::to_value(result).unwrap();

        assert_eq!(value["outcomes"][0]["status"], "success");
        assert_eq!(value["outcomes"][0]["check"]["check"], "size");
    }
}
