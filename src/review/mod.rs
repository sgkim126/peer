use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cache::CacheStore;
use crate::check::{self, CheckCommandError};
use crate::cli::CheckCommand;
use crate::config::Config;
use crate::console::Console;
use crate::context::ReviewContextDigest;
use crate::git::{CommitHash, GitError, run_git};
use crate::llm::{CheckResult, LlmUsage};
use crate::pi::PiRuntime;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewTarget {
    Commit(CommitHash),
    Range {
        from: CommitHash,
        to: CommitHash,
        commits: Vec<CommitHash>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewPlan {
    checks: Vec<ReviewCheck>,
    ordered_commits: Vec<CommitHash>,
    review_head: CommitHash,
}

#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq)]
pub enum ReviewCheckKind {
    Size,
    Intent,
    Quality,
    Security,
    Coherence,
}

impl ReviewPlan {
    pub fn with_only_check(
        mut self,
        selected: &[ReviewCheckKind],
    ) -> Result<Self, ReviewPlanError> {
        if !selected.is_empty() {
            self.checks.retain(|check| selected.contains(&check.kind()));
        }
        self.ensure_not_empty()
    }

    pub fn excluding_check(
        mut self,
        excluded: &[ReviewCheckKind],
    ) -> Result<Self, ReviewPlanError> {
        if !excluded.is_empty() {
            self.checks
                .retain(|check| !excluded.contains(&check.kind()));
        }
        self.ensure_not_empty()
    }

    fn ensure_not_empty(self) -> Result<Self, ReviewPlanError> {
        if self.checks.is_empty() {
            return Err(ReviewPlanError::NoChecksRemaining);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewPlanError {
    NoChecksRemaining,
}

impl fmt::Display for ReviewPlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoChecksRemaining => {
                f.write_str("no checks remain after applying review check filters")
            }
        }
    }
}

impl std::error::Error for ReviewPlanError {}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReviewResult {
    pub summary: ReviewSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_usage: Option<LlmUsage>,
    pub ordered_commits: Vec<CommitHash>,
    pub checks: Vec<CheckResult>,

    #[serde(skip)]
    pub errors: Vec<ReviewCheckError>,
}

impl ReviewResult {
    pub fn is_success(&self) -> bool {
        self.errors.is_empty() && self.checks.iter().all(CheckResult::is_success)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSummary {
    pub peer_version: String,
    pub provider: String,
    pub model: String,
}

pub struct ReviewOptions<'a> {
    pub context_usage: Option<LlmUsage>,
    pub resume: bool,
    pub runtime: &'a mut PiRuntime,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct ModelUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
}

pub fn usage_by_model(
    checks: &[CheckResult],
    context_usage: Option<&LlmUsage>,
) -> BTreeMap<String, ModelUsage> {
    let mut usage_by_model = BTreeMap::<String, ModelUsage>::new();
    if let Some(usage) = context_usage {
        let total = usage_by_model.entry(usage.model.clone()).or_default();
        total.input_tokens += usage.input_tokens;
        total.output_tokens += usage.output_tokens;
        total.cost_usd += usage.cost_usd;
    }
    for check in checks {
        let total = usage_by_model.entry(check.usage.model.clone()).or_default();
        total.input_tokens += check.usage.input_tokens;
        total.output_tokens += check.usage.output_tokens;
        total.cost_usd += check.usage.cost_usd;
    }
    usage_by_model
}

#[derive(Debug)]
pub struct ReviewCheckError {
    pub check: ReviewCheck,
    pub error: CheckCommandError,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewCheck {
    Size { revision: CommitHash },
    Intent { revision: CommitHash },
    Quality { revision: CommitHash },
    Security { revision: CommitHash },
    Coherence { from: CommitHash, to: CommitHash },
}

impl ReviewCheck {
    fn kind(&self) -> ReviewCheckKind {
        match self {
            Self::Size { .. } => ReviewCheckKind::Size,
            Self::Intent { .. } => ReviewCheckKind::Intent,
            Self::Quality { .. } => ReviewCheckKind::Quality,
            Self::Security { .. } => ReviewCheckKind::Security,
            Self::Coherence { .. } => ReviewCheckKind::Coherence,
        }
    }
}

impl From<ReviewCheck> for CheckCommand {
    fn from(check: ReviewCheck) -> Self {
        match check {
            ReviewCheck::Size { revision } => Self::Size {
                revision: revision.to_string(),
            },
            ReviewCheck::Intent { revision } => Self::Intent {
                revision: revision.to_string(),
            },
            ReviewCheck::Quality { revision } => Self::Quality {
                revision: revision.to_string(),
            },
            ReviewCheck::Security { revision } => Self::Security {
                revision: revision.to_string(),
            },
            ReviewCheck::Coherence { from, to } => Self::Coherence {
                range: format!("{from}..{to}"),
            },
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
            Self::Coherence { from, to } => write!(f, "coherence {from}..{to}"),
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

pub async fn resolve_target(
    target: &str,
    max_commits: u32,
    project_root: &Path,
    console: Console,
) -> Result<ReviewTarget, ReviewTargetError> {
    if !target.contains("..") {
        return Ok(ReviewTarget::Commit(
            CommitHash::resolve(target, project_root, console).await?,
        ));
    }

    if target.contains("...") {
        return Err(ReviewTargetError::InvalidRange(target.to_string()));
    }
    let Some((from, to)) = target.split_once("..") else {
        return Err(ReviewTargetError::InvalidRange(target.to_string()));
    };
    if from.is_empty() || to.is_empty() || to.contains("..") {
        return Err(ReviewTargetError::InvalidRange(target.to_string()));
    }

    // Resolve both ends explicitly so invalid revisions produce the same useful
    // error as a single-commit target instead of leaking `git rev-list` stderr.
    let from = CommitHash::resolve(from, project_root, console).await?;
    let to = CommitHash::resolve(to, project_root, console).await?;
    let revision = format!("{from}..{to}");
    let commit_limit = u64::from(max_commits) + 1;
    let output = run_git(
        &[
            "rev-list",
            "--reverse",
            "--max-count",
            &format!("{commit_limit}"),
            &revision,
        ],
        project_root,
        console,
    )
    .await?;
    let commits = output
        .lines()
        .filter(|line| !line.is_empty())
        .map(CommitHash::new)
        .collect::<Result<Vec<_>, _>>()?;
    if commits.len() > max_commits as usize {
        return Err(ReviewTargetError::TooManyCommits {
            actual: commits.len(),
            maximum: max_commits,
        });
    }
    if commits.is_empty() {
        return Err(ReviewTargetError::EmptyRange(target.to_string()));
    }

    Ok(ReviewTarget::Range { from, to, commits })
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

pub fn plan_checks(target: &ReviewTarget) -> ReviewPlan {
    let mut checks = Vec::new();
    let (ordered_commits, review_head) = match target {
        ReviewTarget::Commit(commit) => {
            append_commit_checks(&mut checks, commit);
            (vec![commit.clone()], commit.clone())
        }
        ReviewTarget::Range { from, to, commits } => {
            for commit in commits {
                append_commit_checks(&mut checks, commit);
            }
            checks.push(ReviewCheck::Coherence {
                from: from.clone(),
                to: to.clone(),
            });
            (commits.clone(), to.clone())
        }
    };
    ReviewPlan {
        checks,
        ordered_commits,
        review_head,
    }
}

fn append_commit_checks(checks: &mut Vec<ReviewCheck>, commit: &CommitHash) {
    checks.push(ReviewCheck::Size {
        revision: commit.clone(),
    });
    checks.push(ReviewCheck::Intent {
        revision: commit.clone(),
    });
    checks.push(ReviewCheck::Quality {
        revision: commit.clone(),
    });
    checks.push(ReviewCheck::Security {
        revision: commit.clone(),
    });
}

pub async fn run(
    plan: ReviewPlan,
    console: Console,
    config: &Config,
    project_root: PathBuf,
    cache: &CacheStore,
    review_context: &ReviewContextDigest,
    options: ReviewOptions<'_>,
) -> ReviewResult {
    let ReviewOptions {
        context_usage,
        resume,
        runtime,
    } = options;
    let ReviewPlan {
        checks: planned_checks,
        ordered_commits,
        review_head,
    } = plan;
    let summary = ReviewSummary {
        peer_version: env!("CARGO_PKG_VERSION").to_string(),
        provider: config.llm.default_provider.clone(),
        model: config.llm.default_model.clone(),
    };
    let mut checks = Vec::with_capacity(planned_checks.len());
    let mut errors = Vec::new();

    // Checks are intentionally ordered: output follows commit order and the
    // range-level coherence check runs after all per-commit checks.
    for review_check in planned_checks {
        let command = CheckCommand::from(review_check.clone());
        match check::handler(
            console,
            command,
            config,
            project_root.clone(),
            cache,
            review_context,
            check::CheckOptions {
                context_usage: None,
                resume,
                review_head: review_head.clone(),
                runtime,
            },
        )
        .await
        {
            Ok(result) => checks.push(result),
            Err(error) => errors.push(ReviewCheckError {
                check: review_check,
                error,
            }),
        }
    }

    ReviewResult {
        summary,
        context_usage,
        ordered_commits,
        checks,
        errors,
    }
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
            Self::InvalidRange(range) => write!(f, "{range} is not a two-dot range"),
            Self::EmptyRange(range) => write!(f, "{range} contains no commits"),
            Self::TooManyCommits { actual, maximum } => {
                write!(
                    f,
                    "review target contains at least {actual} commits (max: {maximum})"
                )
            }
            Self::MergeCommit(commit) => write!(f, "review target contains merge commit {commit}"),
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
    use super::*;

    use std::assert_matches;

    use tempfile::TempDir;

    use crate::llm::{CheckError, CheckTarget};

    struct Repo {
        _tmp: TempDir,
        path: PathBuf,
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
            std::fs::write(self.path.join(file), message).unwrap();
            run_git(&["add", file], &self.path, Console::default())
                .await
                .unwrap();
            run_git(
                &["commit", "--no-gpg-sign", "-m", message],
                &self.path,
                Console::default(),
            )
            .await
            .unwrap();
            CommitHash::resolve("HEAD", &self.path, Console::default())
                .await
                .unwrap()
        }
    }

    fn review_result(check_error: Option<CheckError>) -> ReviewResult {
        let commit = CommitHash::new("abc1234").unwrap();
        ReviewResult {
            summary: ReviewSummary {
                peer_version: "test".to_string(),
                provider: "test".to_string(),
                model: "test".to_string(),
            },
            context_usage: None,
            ordered_commits: vec![commit.clone()],
            checks: vec![CheckResult {
                check: "quality".to_string(),
                target: CheckTarget::Commit(commit.clone()),
                ordered_commits: vec![commit],
                summary: "Checked.".to_string(),
                findings: Vec::new(),
                iterations: 1,
                error: check_error,
                context_usage: None,
                usage: LlmUsage {
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                    cost_usd: 0.0,
                    model: "test".to_string(),
                    models: Vec::new(),
                },
            }],
            errors: Vec::new(),
        }
    }

    #[test]
    fn review_succeeds_only_when_all_checks_succeed() {
        assert!(review_result(None).is_success());
        assert!(
            !review_result(Some(CheckError::ClarificationRequired {
                questions: vec!["Which deployment policy applies?".to_string()],
            }))
            .is_success()
        );
    }

    #[tokio::test]
    async fn resolves_range_oldest_to_newest() {
        let repo = Repo::new().await;
        let base = repo.commit("a.txt", "first").await;
        let second = repo.commit("b.txt", "second").await;
        let third = repo.commit("c.txt", "third").await;
        let target = format!("{base}..HEAD");

        assert_eq!(
            resolve_target(&target, 10, &repo.path, Console::default())
                .await
                .unwrap(),
            ReviewTarget::Range {
                from: base,
                to: third.clone(),
                commits: vec![second, third],
            }
        );
    }

    #[tokio::test]
    async fn rejects_invalid_and_empty_ranges() {
        let repo = Repo::new().await;
        repo.commit("a.txt", "first").await;

        assert_matches!(
            resolve_target("HEAD...HEAD", 10, &repo.path, Console::default()).await,
            Err(ReviewTargetError::InvalidRange(_))
        );
        assert_matches!(
            resolve_target("HEAD..HEAD", 10, &repo.path, Console::default()).await,
            Err(ReviewTargetError::EmptyRange(_))
        );
    }

    #[tokio::test]
    async fn rejects_oversized_ranges_while_resolving() {
        let repo = Repo::new().await;
        let base = repo.commit("a.txt", "first").await;
        repo.commit("b.txt", "second").await;
        repo.commit("c.txt", "third").await;
        let target = format!("{base}..HEAD");

        assert_matches!(
            resolve_target(&target, 1, &repo.path, Console::default()).await,
            Err(ReviewTargetError::TooManyCommits {
                actual: 2,
                maximum: 1
            })
        );
    }

    #[tokio::test]
    async fn enforces_maximum_commit_count() {
        let repo = Repo::new().await;
        let first = repo.commit("a.txt", "first").await;
        let second = repo.commit("b.txt", "second").await;
        let target = ReviewTarget::Range {
            from: first.clone(),
            to: second.clone(),
            commits: vec![first, second],
        };

        assert_matches!(
            validate_target(&target, 1, &repo.path, Console::default()).await,
            Err(ReviewTargetError::TooManyCommits {
                actual: 2,
                maximum: 1
            })
        );
    }

    #[test]
    fn plans_commit_checks_and_range_coherence() {
        let first = CommitHash::new("abc1234").unwrap();
        let second = CommitHash::new("def5678").unwrap();
        let plan = plan_checks(&ReviewTarget::Range {
            from: first.clone(),
            to: second.clone(),
            commits: vec![first.clone(), second.clone()],
        });

        assert_eq!(plan.checks.len(), 9);
        assert_eq!(
            plan.checks[2],
            ReviewCheck::Quality {
                revision: first.clone(),
            }
        );
        assert_eq!(
            plan.checks[6],
            ReviewCheck::Quality {
                revision: second.clone(),
            }
        );
        assert_eq!(plan.review_head, second.clone());
        assert_eq!(
            plan.checks.last(),
            Some(&ReviewCheck::Coherence {
                from: first,
                to: second,
            })
        );
    }

    #[test]
    fn uses_the_target_commit_as_the_review_head_for_a_single_commit() {
        let commit = CommitHash::new("abc1234").unwrap();
        let plan = plan_checks(&ReviewTarget::Commit(commit.clone()));

        assert_eq!(
            plan.checks[2],
            ReviewCheck::Quality {
                revision: commit.clone(),
            }
        );
        assert_eq!(plan.review_head, commit);
    }

    #[test]
    fn filters_plan_to_selected_check_kinds() {
        let first = CommitHash::new("abc1234").unwrap();
        let second = CommitHash::new("def5678").unwrap();
        let plan = plan_checks(&ReviewTarget::Range {
            from: first.clone(),
            to: second.clone(),
            commits: vec![first, second],
        })
        .with_only_check(&[ReviewCheckKind::Intent, ReviewCheckKind::Coherence])
        .unwrap();

        assert_eq!(plan.checks.len(), 3);
        assert_eq!(
            plan.checks
                .iter()
                .map(ReviewCheck::kind)
                .collect::<Vec<_>>(),
            [
                ReviewCheckKind::Intent,
                ReviewCheckKind::Intent,
                ReviewCheckKind::Coherence,
            ]
        );
    }

    #[test]
    fn removes_skipped_check_kinds_from_plan() {
        let first = CommitHash::new("abc1234").unwrap();
        let second = CommitHash::new("def5678").unwrap();
        let plan = plan_checks(&ReviewTarget::Range {
            from: first.clone(),
            to: second.clone(),
            commits: vec![first, second],
        })
        .excluding_check(&[ReviewCheckKind::Quality, ReviewCheckKind::Coherence])
        .unwrap();

        assert_eq!(plan.checks.len(), 6);
        for check in plan.checks {
            assert_matches!(
                check.kind(),
                ReviewCheckKind::Size | ReviewCheckKind::Intent | ReviewCheckKind::Security
            );
        }
    }

    #[test]
    fn rejects_plan_when_skip_checks_remove_every_applicable_check() {
        let commit = CommitHash::new("abc1234").unwrap();
        let result = plan_checks(&ReviewTarget::Commit(commit)).excluding_check(&[
            ReviewCheckKind::Size,
            ReviewCheckKind::Intent,
            ReviewCheckKind::Quality,
            ReviewCheckKind::Security,
            ReviewCheckKind::Coherence,
        ]);

        assert_eq!(result.unwrap_err(), ReviewPlanError::NoChecksRemaining);
    }

    #[test]
    fn rejects_plan_when_only_checks_are_not_applicable() {
        let commit = CommitHash::new("abc1234").unwrap();
        let result = plan_checks(&ReviewTarget::Commit(commit))
            .with_only_check(&[ReviewCheckKind::Coherence]);

        assert_eq!(result.unwrap_err(), ReviewPlanError::NoChecksRemaining);
    }

    #[test]
    fn accepts_plan_when_skip_checks_leave_an_applicable_check() {
        let commit = CommitHash::new("abc1234").unwrap();
        let plan = plan_checks(&ReviewTarget::Commit(commit))
            .excluding_check(&[
                ReviewCheckKind::Size,
                ReviewCheckKind::Intent,
                ReviewCheckKind::Quality,
            ])
            .unwrap();

        assert_eq!(plan.checks.len(), 1);
        assert_eq!(plan.checks[0].kind(), ReviewCheckKind::Security);
    }
}
