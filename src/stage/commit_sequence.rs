use serde::{Deserialize, Serialize};

use crate::git::CommitHash;
use crate::review::ReviewInput;
use crate::stage::StageTarget;
use crate::stage::commit_scope::CommitScopeReport;
use crate::stage::contract::{ReviewStage, StageKind, StageRequest};
use crate::stage::review_context::ReviewContextReport;

const SYSTEM_PROMPT: &str = concat!(
    "You are assessing the order and progression of commits whose pull-request scope has already been classified. ",
    "Use the supplied review context, scope report, ordered messages, and per-commit diffs as authoritative. ",
    "Summarize the direction of every commit, identify its dependencies, and mark forward work, fixups, or reversions. ",
    "Report reorder, dependency-direction, or confusing-progression issues. ",
    "Do not revisit whether a commit belongs in the pull request, recommend separate pull requests, assess per-commit atomicity, or recommend splitting or squashing. ",
    "Do not assess intent accuracy, code quality, or security."
);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SequenceChangeKind {
    Forward,
    Fixup,
    Reversion,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CommitProgress {
    pub commit: CommitHash,
    pub direction: String,
    pub change_kind: SequenceChangeKind,
    pub depends_on: Vec<CommitHash>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SequenceIssueKind {
    Reorder,
    DependencyDirection,
    ConfusingProgression,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SequenceIssue {
    pub kind: SequenceIssueKind,
    pub commits: Vec<CommitHash>,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CommitSequenceReport {
    pub summary: String,
    pub progression: Vec<CommitProgress>,
    pub issues: Vec<SequenceIssue>,
}

pub struct CommitSequenceStage {
    input: ReviewInput,
    context: ReviewContextReport,
    scope: CommitScopeReport,
    commits: Vec<CommitHash>,
    target: StageTarget,
}

impl CommitSequenceStage {
    #[expect(dead_code)]
    pub fn new(input: ReviewInput, context: ReviewContextReport, scope: CommitScopeReport) -> Self {
        let commits = input
            .commits
            .iter()
            .map(|commit| commit.hash.clone())
            .collect();
        let target = match &input.base {
            Some(base) => StageTarget::Range {
                from: base.clone(),
                to: input.head.clone(),
            },
            None => StageTarget::Commit(input.head.clone()),
        };
        Self {
            input,
            context,
            scope,
            commits,
            target,
        }
    }

    fn contains_commit(&self, commit: &CommitHash) -> bool {
        self.commits.iter().any(|expected| expected.matches(commit))
    }
}

impl ReviewStage for CommitSequenceStage {
    type Report = CommitSequenceReport;

    fn kind(&self) -> StageKind {
        StageKind::CommitSequence
    }

    fn target(&self) -> StageTarget {
        self.target.clone()
    }

    fn expected_commits(&self) -> &[CommitHash] {
        &self.commits
    }

    fn request(&self) -> StageRequest {
        let commits: Vec<_> = self
            .input
            .commits
            .iter()
            .map(|commit| {
                serde_json::json!({
                    "commit": commit.hash,
                    "message": commit.message,
                    "diff": commit.diff,
                })
            })
            .collect();
        let input = serde_json::json!({
            "review_context": self.context,
            "commit_scope": self.scope,
            "commits_oldest_to_newest": commits,
        });
        StageRequest {
            system_prompt: SYSTEM_PROMPT.to_string(),
            prompt: format!(
                "Assess the commit sequence:\n{}",
                serde_json::to_string_pretty(&input).expect("commit sequence input serializes")
            ),
            read_tools: Vec::new(),
        }
    }

    fn validate_report(&self, report: &Self::Report) -> Result<(), String> {
        if report.summary.trim().is_empty() {
            return Err("commit sequence summary must not be empty".to_string());
        }
        if report.progression.len() != self.commits.len() {
            return Err("commit sequence must describe every target commit".to_string());
        }
        for (progress, expected) in report.progression.iter().zip(&self.commits) {
            if !expected.matches(&progress.commit) {
                return Err(format!(
                    "commit sequence entry {} is out of order or outside the target",
                    progress.commit
                ));
            }
            if progress.direction.trim().is_empty() {
                return Err("commit sequence directions must not be blank".to_string());
            }
            if progress
                .depends_on
                .iter()
                .any(|dependency| expected.matches(dependency))
            {
                return Err(format!(
                    "sequence dependency {} must not reference itself",
                    progress.commit
                ));
            }
            if let Some(commit) = progress
                .depends_on
                .iter()
                .find(|commit| !self.contains_commit(commit))
            {
                return Err(format!(
                    "sequence dependency {commit} is outside the target"
                ));
            }
        }
        for issue in &report.issues {
            if issue.message.trim().is_empty() || issue.commits.is_empty() {
                return Err("sequence issues require commits and a message".to_string());
            }
            if let Some(commit) = issue
                .commits
                .iter()
                .find(|commit| !self.contains_commit(commit))
            {
                return Err(format!(
                    "sequence issue commit {commit} is outside the target"
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stage() -> CommitSequenceStage {
        let commit = CommitHash::new("abc123456789").unwrap();
        CommitSequenceStage {
            input: ReviewInput {
                context: crate::context::ReviewContext::default(),
                base: None,
                head: commit.clone(),
                commits: Vec::new(),
                cumulative_diff: String::new(),
            },
            context: ReviewContextReport {
                summary: "Context".to_string(),
                objectives: Vec::new(),
                expected_behavior: Vec::new(),
                scope: Vec::new(),
                constraints: Vec::new(),
                implementation: Vec::new(),
                verification: Vec::new(),
                unresolved: Vec::new(),
            },
            scope: CommitScopeReport {
                summary: "Scope".to_string(),
                commits: Vec::new(),
            },
            commits: vec![commit.clone()],
            target: StageTarget::Commit(commit),
        }
    }

    #[test]
    fn rejects_dependencies_outside_the_target() {
        let commit = CommitHash::new("abc1234").unwrap();
        let report = CommitSequenceReport {
            summary: "Sequence".to_string(),
            progression: vec![CommitProgress {
                commit,
                direction: "Add the foundation".to_string(),
                change_kind: SequenceChangeKind::Forward,
                depends_on: vec![CommitHash::new("def5678").unwrap()],
            }],
            issues: Vec::new(),
        };

        assert_eq!(
            stage().validate_report(&report),
            Err("sequence dependency def5678 is outside the target".to_string())
        );
    }

    #[test]
    fn rejects_self_dependencies() {
        let commit = CommitHash::new("abc1234").unwrap();
        let report = CommitSequenceReport {
            summary: "Sequence".to_string(),
            progression: vec![CommitProgress {
                commit: commit.clone(),
                direction: "Add the foundation".to_string(),
                change_kind: SequenceChangeKind::Forward,
                depends_on: vec![commit],
            }],
            issues: Vec::new(),
        };

        assert_eq!(
            stage().validate_report(&report),
            Err("sequence dependency abc1234 must not reference itself".to_string())
        );
    }

    #[test]
    fn allows_a_dependency_on_a_later_commit_for_direction_reporting() {
        let first = CommitHash::new("abc1234").unwrap();
        let second = CommitHash::new("def5678").unwrap();
        let mut stage = stage();
        stage.commits = vec![first.clone(), second.clone()];
        let report = CommitSequenceReport {
            summary: "Sequence".to_string(),
            progression: vec![
                CommitProgress {
                    commit: first,
                    direction: "Use a facility introduced later".to_string(),
                    change_kind: SequenceChangeKind::Forward,
                    depends_on: vec![second.clone()],
                },
                CommitProgress {
                    commit: second,
                    direction: "Introduce the required facility".to_string(),
                    change_kind: SequenceChangeKind::Forward,
                    depends_on: Vec::new(),
                },
            ],
            issues: vec![SequenceIssue {
                kind: SequenceIssueKind::DependencyDirection,
                commits: vec![CommitHash::new("abc1234").unwrap()],
                message: "The dependency is introduced after its consumer".to_string(),
            }],
        };

        assert_eq!(stage.validate_report(&report), Ok(()));
    }
}
