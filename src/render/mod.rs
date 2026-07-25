mod github;
mod markdown;
mod terminal;

use std::collections::BTreeMap;
use std::fmt;
use std::io::IsTerminal;

use serde::{Deserialize, Serialize};

use crate::cli::OutputFormat;
use crate::git::CommitHash;
use crate::llm::result::{CheckError, CheckResult, CheckTarget, Finding, LlmUsage};
use crate::review::{ReviewCheck, ReviewCheckError, ReviewResult, ReviewSummary};

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct RenderDocument {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<ReviewSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_usage: Option<LlmUsage>,
    pub ordered_commits: Vec<CommitHash>,
    pub checks: Vec<RenderCheck>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct RenderCheck {
    pub check: String,
    pub target: CheckTarget,
    pub outcome: RenderCheckOutcome,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RenderCheckOutcome {
    Clean {
        summary: String,
        iterations: u32,
        usage: LlmUsage,
    },
    Issues {
        summary: String,
        findings: Vec<Finding>,
        iterations: u32,
        usage: LlmUsage,
    },
    Exhausted {
        summary: String,
        findings: Vec<Finding>,
        reason: String,
        iterations: u32,
        usage: LlmUsage,
    },
    Failed {
        failure: RenderCheckFailure,
    },
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RenderCheckFailure {
    Check {
        summary: String,
        findings: Vec<Finding>,
        error: CheckError,
        iterations: u32,
        usage: LlmUsage,
    },
    Execution {
        reason: String,
    },
}

enum RenderCheckErrorRef<'a> {
    Exhausted(&'a str),
    Check(&'a CheckError),
    Execution(&'a str),
}

impl RenderCheck {
    fn status(&self) -> &'static str {
        match self.outcome {
            RenderCheckOutcome::Clean { .. } => "clean",
            RenderCheckOutcome::Issues { .. } => "issues",
            RenderCheckOutcome::Exhausted { .. } => "exhausted",
            RenderCheckOutcome::Failed { .. } => "failed",
        }
    }

    fn summary(&self) -> Option<&str> {
        match &self.outcome {
            RenderCheckOutcome::Clean { summary, .. } => Some(summary),
            RenderCheckOutcome::Issues { summary, .. } => Some(summary),
            RenderCheckOutcome::Exhausted { summary, .. } => Some(summary),
            RenderCheckOutcome::Failed {
                failure: RenderCheckFailure::Check { summary, .. },
            } => Some(summary),
            RenderCheckOutcome::Failed {
                failure: RenderCheckFailure::Execution { .. },
            } => None,
        }
    }

    fn findings(&self) -> &[Finding] {
        match &self.outcome {
            RenderCheckOutcome::Issues { findings, .. } => findings,
            RenderCheckOutcome::Exhausted { findings, .. } => findings,
            RenderCheckOutcome::Failed {
                failure: RenderCheckFailure::Check { findings, .. },
            } => findings,
            RenderCheckOutcome::Clean { .. } => &[],
            RenderCheckOutcome::Failed {
                failure: RenderCheckFailure::Execution { .. },
            } => &[],
        }
    }

    fn iterations(&self) -> Option<u32> {
        match &self.outcome {
            RenderCheckOutcome::Clean { iterations, .. } => Some(*iterations),
            RenderCheckOutcome::Issues { iterations, .. } => Some(*iterations),
            RenderCheckOutcome::Exhausted { iterations, .. } => Some(*iterations),
            RenderCheckOutcome::Failed {
                failure: RenderCheckFailure::Check { iterations, .. },
            } => Some(*iterations),
            RenderCheckOutcome::Failed {
                failure: RenderCheckFailure::Execution { .. },
            } => None,
        }
    }

    fn usage(&self) -> Option<&LlmUsage> {
        match &self.outcome {
            RenderCheckOutcome::Clean { usage, .. } => Some(usage),
            RenderCheckOutcome::Issues { usage, .. } => Some(usage),
            RenderCheckOutcome::Exhausted { usage, .. } => Some(usage),
            RenderCheckOutcome::Failed {
                failure: RenderCheckFailure::Check { usage, .. },
            } => Some(usage),
            RenderCheckOutcome::Failed {
                failure: RenderCheckFailure::Execution { .. },
            } => None,
        }
    }

    fn error(&self) -> Option<RenderCheckErrorRef<'_>> {
        match &self.outcome {
            RenderCheckOutcome::Exhausted { reason, .. } => {
                Some(RenderCheckErrorRef::Exhausted(reason))
            }
            RenderCheckOutcome::Failed {
                failure: RenderCheckFailure::Check { error, .. },
            } => Some(RenderCheckErrorRef::Check(error)),
            RenderCheckOutcome::Failed {
                failure: RenderCheckFailure::Execution { reason },
            } => Some(RenderCheckErrorRef::Execution(reason)),
            RenderCheckOutcome::Clean { .. } => None,
            RenderCheckOutcome::Issues { .. } => None,
        }
    }
}

impl From<CheckResult> for RenderDocument {
    fn from(mut result: CheckResult) -> Self {
        let context_usage = result.context_usage.take();
        let ordered_commits = std::mem::take(&mut result.ordered_commits);
        Self {
            summary: None,
            context_usage,
            ordered_commits,
            checks: vec![result.into()],
        }
    }
}

impl From<ReviewResult> for RenderDocument {
    fn from(review: ReviewResult) -> Self {
        let ReviewResult {
            summary,
            context_usage,
            ordered_commits,
            checks,
            errors,
        } = review;
        let mut checks = checks
            .into_iter()
            .map(RenderCheck::from)
            .chain(errors.into_iter().map(RenderCheck::from))
            .collect::<Vec<_>>();
        checks.sort_by_key(|check| render_check_order(check, &ordered_commits));
        Self {
            summary: Some(summary),
            context_usage,
            ordered_commits,
            checks,
        }
    }
}

impl From<CheckResult> for RenderCheck {
    fn from(result: CheckResult) -> Self {
        let CheckResult {
            check,
            target,
            ordered_commits,
            summary,
            mut findings,
            iterations,
            error,
            context_usage: _,
            usage,
        } = result;
        findings.sort_by_key(|finding| {
            ordered_commits
                .iter()
                .position(|commit| commit.matches(&finding.commit))
                .unwrap_or(usize::MAX)
        });
        let outcome = match error {
            None if findings.is_empty() => RenderCheckOutcome::Clean {
                summary,
                iterations,
                usage,
            },
            None => RenderCheckOutcome::Issues {
                summary,
                findings,
                iterations,
                usage,
            },
            Some(CheckError::Exhausted { reason }) => RenderCheckOutcome::Exhausted {
                summary,
                findings,
                reason,
                iterations,
                usage,
            },
            Some(error) => RenderCheckOutcome::Failed {
                failure: RenderCheckFailure::Check {
                    summary,
                    findings,
                    error,
                    iterations,
                    usage,
                },
            },
        };
        Self {
            check,
            target,
            outcome,
        }
    }
}

impl From<ReviewCheckError> for RenderCheck {
    fn from(failure: ReviewCheckError) -> Self {
        let (check, target) = match failure.check {
            ReviewCheck::Size { revision } => ("size", CheckTarget::Commit(revision)),
            ReviewCheck::Intent { revision } => ("intent", CheckTarget::Commit(revision)),
            ReviewCheck::Quality { revision } => ("quality", CheckTarget::Commit(revision)),
            ReviewCheck::Security { revision } => ("security", CheckTarget::Commit(revision)),
            ReviewCheck::Coherence { from, to } => ("coherence", CheckTarget::Range { from, to }),
        };
        Self {
            check: check.to_string(),
            target,
            outcome: RenderCheckOutcome::Failed {
                failure: RenderCheckFailure::Execution {
                    reason: failure.error.to_string(),
                },
            },
        }
    }
}

fn render_check_order(check: &RenderCheck, ordered_commits: &[CommitHash]) -> (usize, usize) {
    let check_order = match check.check.as_str() {
        "size" => 0,
        "intent" => 1,
        "quality" => 2,
        "security" => 3,
        "coherence" => 4,
        _ => usize::MAX,
    };
    let commit_order = match &check.target {
        CheckTarget::Commit(target) => ordered_commits
            .iter()
            .position(|commit| commit.matches(target))
            .unwrap_or(usize::MAX),
        CheckTarget::Range { .. } => usize::MAX,
    };
    (check_order, commit_order)
}

fn escape_markdown(value: &str) -> String {
    let value = single_line(value);
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(
            character,
            '\\' | '`'
                | '*'
                | '_'
                | '{'
                | '}'
                | '['
                | ']'
                | '<'
                | '>'
                | '('
                | ')'
                | '#'
                | '+'
                | '-'
                | '.'
                | '!'
                | '|'
                | '~'
        ) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

fn escape_html(value: &str) -> String {
    single_line(value)
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn escape_terminal(value: &str) -> String {
    let value = single_line(value);
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_control() {
            escaped.extend(character.escape_default());
        } else {
            escaped.push(character);
        }
    }
    escaped
}

fn single_line(value: &str) -> String {
    value.replace("\r\n", " ").replace(['\r', '\n'], " ")
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderOptions {
    format: RenderFormat,
}

#[derive(Clone, Debug, PartialEq)]
enum RenderFormat {
    Json,
    Terminal,
    Markdown,
    Github { repo: String },
}

impl RenderOptions {
    pub fn from_cli(
        format: OutputFormat,
        repo: Option<String>,
    ) -> Result<Self, RenderOptionsError> {
        match (format, repo) {
            (OutputFormat::Json, None) => Ok(Self {
                format: RenderFormat::Json,
            }),
            (OutputFormat::Terminal, None) => Ok(Self {
                format: RenderFormat::Terminal,
            }),
            (OutputFormat::Markdown, None) => Ok(Self {
                format: RenderFormat::Markdown,
            }),
            (OutputFormat::Github, Some(repo)) => {
                validate_github_repo(&repo)?;
                Ok(Self {
                    format: RenderFormat::Github { repo },
                })
            }
            (OutputFormat::Github, None) => Err(RenderOptionsError::GithubRepoRequired),
            (_, Some(_)) => Err(RenderOptionsError::RepoRequiresGithubFormat),
        }
    }
}

pub fn render(document: RenderDocument, options: RenderOptions) -> Result<String, RenderError> {
    match options.format {
        RenderFormat::Json => {
            serde_json::to_string_pretty(&document).map_err(RenderError::Serialization)
        }
        RenderFormat::Terminal => {
            let use_color = std::io::stdout().is_terminal();
            let checks = document
                .checks
                .iter()
                .map(|result| terminal::render(result, use_color))
                .collect::<Vec<_>>()
                .join("\n\n");
            let summary = document.summary.as_ref().map(|summary| {
                terminal::render_review_summary(
                    summary,
                    document.context_usage.as_ref(),
                    &usage_by_model(&document),
                    use_color,
                )
            });
            let context = document
                .summary
                .is_none()
                .then_some(document.context_usage.as_ref())
                .flatten()
                .map(terminal::render_context_usage);
            Ok(join_review_sections(
                summary.into_iter().chain(context).chain([checks]),
            ))
        }
        RenderFormat::Markdown => {
            let checks = document
                .checks
                .iter()
                .map(markdown::render)
                .collect::<Vec<_>>()
                .join("\n\n");
            let summary = document.summary.as_ref().map(|summary| {
                markdown::render_review_summary(
                    summary,
                    document.context_usage.as_ref(),
                    &usage_by_model(&document),
                )
            });
            let context = document
                .summary
                .is_none()
                .then_some(document.context_usage.as_ref())
                .flatten()
                .map(markdown::render_context_usage);
            Ok(join_review_sections(
                summary.into_iter().chain(context).chain([checks]),
            ))
        }
        RenderFormat::Github { repo } => {
            let checks = document
                .checks
                .iter()
                .map(|result| github::render(result, &repo))
                .collect::<Vec<_>>()
                .join("\n\n");
            let summary = document.summary.as_ref().map(|summary| {
                github::render_review_summary(
                    summary,
                    document.context_usage.as_ref(),
                    &usage_by_model(&document),
                )
            });
            let context = document
                .summary
                .is_none()
                .then_some(document.context_usage.as_ref())
                .flatten()
                .map(github::render_context_usage);
            Ok(join_review_sections(
                summary.into_iter().chain(context).chain([checks]),
            ))
        }
    }
}

fn usage_by_model(document: &RenderDocument) -> BTreeMap<String, crate::review::ModelUsage> {
    let mut usage = BTreeMap::new();
    for item in document
        .context_usage
        .iter()
        .chain(document.checks.iter().filter_map(RenderCheck::usage))
    {
        let total = usage
            .entry(item.model.clone())
            .or_insert_with(crate::review::ModelUsage::default);
        total.input_tokens += item.input_tokens;
        total.output_tokens += item.output_tokens;
        total.cost_usd += item.cost_usd;
    }
    usage
}

fn join_review_sections(sections: impl IntoIterator<Item = String>) -> String {
    sections
        .into_iter()
        .filter(|section| !section.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn validate_github_repo(repo: &str) -> Result<(), RenderOptionsError> {
    let Some((owner, name)) = repo.split_once('/') else {
        return Err(RenderOptionsError::MalformedRepo);
    };
    if owner.is_empty()
        || name.is_empty()
        || name.contains('/')
        || !owner.chars().all(is_github_repo_char)
        || !name.chars().all(is_github_repo_char)
    {
        return Err(RenderOptionsError::MalformedRepo);
    }
    Ok(())
}

fn is_github_repo_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-')
}

#[derive(Debug)]
pub enum RenderError {
    Serialization(serde_json::Error),
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serialization(error) => write!(f, "cannot serialize render document: {error}"),
        }
    }
}

impl std::error::Error for RenderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Serialization(error) => Some(error),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum RenderOptionsError {
    GithubRepoRequired,
    RepoRequiresGithubFormat,
    MalformedRepo,
}

impl fmt::Display for RenderOptionsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GithubRepoRequired => write!(f, "--format github requires --repo <owner/name>"),
            Self::RepoRequiresGithubFormat => {
                write!(f, "--repo can only be used with --format github")
            }
            Self::MalformedRepo => write!(f, "--repo must use the form owner/name"),
        }
    }
}

impl std::error::Error for RenderOptionsError {}

#[cfg(test)]
mod tests {
    use std::assert_matches;

    use super::*;
    use crate::git::CommitHash;
    use crate::llm::result::{CheckTarget, FileLocation, Finding, LlmUsage, Severity};

    fn result() -> CheckResult {
        CheckResult {
            check: "security".to_string(),
            target: CheckTarget::Range {
                from: CommitHash::new("abc1234").unwrap(),
                to: CommitHash::new("def5678").unwrap(),
            },
            ordered_commits: vec![
                CommitHash::new("abc1234").unwrap(),
                CommitHash::new("def5678").unwrap(),
            ],
            summary: "Checked the change.".to_string(),
            findings: vec![
                Finding {
                    commit: CommitHash::new("def5678").unwrap(),
                    severity: Severity::Info,
                    message: "Informational finding.".to_string(),
                    location: None,
                },
                Finding {
                    commit: CommitHash::new("abc1234").unwrap(),
                    severity: Severity::High,
                    message: "High-risk finding.".to_string(),
                    location: Some(FileLocation {
                        file: "src/main.rs".to_string(),
                        line: Some(42),
                    }),
                },
            ],
            iterations: 2,
            error: None,
            context_usage: None,
            usage: LlmUsage {
                input_tokens: 100,
                output_tokens: 20,
                cost_usd: 0.001,
                model: "test-model".to_string(),
            },
        }
    }

    fn review_summary() -> crate::review::ReviewSummary {
        crate::review::ReviewSummary {
            peer_version: "0.1.0".to_string(),
            provider: "test-provider".to_string(),
            model: "test-model".to_string(),
        }
    }

    fn review_context_usage() -> LlmUsage {
        LlmUsage {
            input_tokens: 40,
            output_tokens: 10,
            cost_usd: 0.0004,
            model: "test-model".to_string(),
        }
    }

    fn review_ordered_commits() -> Vec<CommitHash> {
        vec![
            CommitHash::new("abc1234").unwrap(),
            CommitHash::new("def5678").unwrap(),
        ]
    }

    #[test]
    fn orders_findings_with_abbreviated_commit_hashes() {
        let mut result = result();
        result.ordered_commits = vec![
            CommitHash::new(&format!("abc1234{}", "0".repeat(33))).unwrap(),
            CommitHash::new(&format!("def5678{}", "0".repeat(33))).unwrap(),
        ];

        let result = RenderCheck::from(result);
        assert_eq!(result.findings()[0].commit.as_ref(), "abc1234");
        assert_eq!(result.findings()[1].commit.as_ref(), "def5678");
    }

    #[test]
    fn render_orders_findings_by_commit_order() {
        let options = RenderOptions::from_cli(OutputFormat::Markdown, None).unwrap();
        let output = render(result().into(), options).unwrap();

        assert!(output.find("abc1234").unwrap() < output.find("def5678").unwrap());
    }

    #[test]
    fn serializes_review_results_as_ordered_checks() {
        let mut second = result();
        second.target = CheckTarget::Commit(CommitHash::new("fedcba9").unwrap());
        let review = crate::review::ReviewResult {
            summary: review_summary(),
            context_usage: None,
            ordered_commits: review_ordered_commits(),
            checks: vec![result(), second],
            errors: Vec::new(),
        };
        let options = RenderOptions::from_cli(OutputFormat::Json, None).unwrap();

        let output = render(review.into(), options).unwrap();
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();

        assert!(output.starts_with("{\n  \"summary\""));
        assert_eq!(value["summary"]["peer_version"], "0.1.0");
        assert_eq!(value["summary"]["provider"], "test-provider");
        assert_eq!(value["summary"]["model"], "test-model");
        assert!(value["summary"].get("usage_by_model").is_none());
        assert_eq!(value["checks"].as_array().unwrap().len(), 2);
        assert_eq!(value["checks"][0]["check"], "security");
        assert_eq!(
            value["checks"][0]["outcome"]["findings"][0]["commit"],
            "abc1234"
        );
        assert_eq!(value["checks"][1]["target"], "fedcba9");
    }

    #[test]
    fn renders_review_context_usage_once_and_includes_it_in_totals() {
        for (format, repo, expected_context_usage, expected_total_usage) in [
            (
                OutputFormat::Terminal,
                None,
                "- Context usage: 40 input, 10 output, $0.000400 (test-model)",
                "  - test-model: 240 input, 50 output, $0.002400",
            ),
            (
                OutputFormat::Markdown,
                None,
                "- **Context usage:** 40 input tokens, 10 output tokens, $0.000400 (test\\-model)",
                "- **test\\-model:** 240 input tokens, 50 output tokens, $0.002400",
            ),
            (
                OutputFormat::Github,
                Some("owner/repo".to_string()),
                "- **Context usage:** 40 input tokens, 10 output tokens, $0.000400 (test\\-model)",
                "- **test\\-model:** 240 input tokens, 50 output tokens, $0.002400",
            ),
        ] {
            let mut second = result();
            second.check = "quality".into();
            let review = crate::review::ReviewResult {
                summary: review_summary(),
                context_usage: Some(review_context_usage()),
                ordered_commits: review_ordered_commits(),
                checks: vec![result(), second],
                errors: Vec::new(),
            };
            let options = RenderOptions::from_cli(format, repo).unwrap();

            let output = render(review.into(), options).unwrap();

            assert_eq!(output.matches(expected_context_usage).count(), 1);
            assert!(output.contains(expected_total_usage));
        }
    }

    #[test]
    fn includes_review_context_usage_in_json_output() {
        let review = crate::review::ReviewResult {
            summary: review_summary(),
            context_usage: Some(review_context_usage()),
            ordered_commits: review_ordered_commits(),
            checks: vec![result()],
            errors: Vec::new(),
        };
        let options = RenderOptions::from_cli(OutputFormat::Json, None).unwrap();

        let output = render(review.into(), options).unwrap();
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();

        assert_eq!(value["context_usage"]["input_tokens"], 40);
        assert_eq!(value["context_usage"]["output_tokens"], 10);
        assert_eq!(value["context_usage"]["model"], "test-model");
        assert!(value["checks"][0].get("context_usage").is_none());
    }

    #[test]
    fn renders_review_summary_before_checks_in_human_readable_formats() {
        let mut second = result();
        second.check = "quality".into();
        for (format, repo, expected_usage, expected_provider) in [
            (
                OutputFormat::Terminal,
                None,
                "  - test-model: 200 input, 40 output, $0.002000",
                "- Provider: test-provider",
            ),
            (
                OutputFormat::Markdown,
                None,
                "- **test\\-model:** 200 input tokens, 40 output tokens, $0.002000",
                "- **Provider:** test-provider",
            ),
            (
                OutputFormat::Github,
                Some("owner/repo".to_string()),
                "- **test\\-model:** 200 input tokens, 40 output tokens, $0.002000",
                "- **Provider:** test-provider",
            ),
        ] {
            let review = crate::review::ReviewResult {
                summary: review_summary(),
                context_usage: None,
                ordered_commits: review_ordered_commits(),
                checks: vec![result(), second.clone()],
                errors: Vec::new(),
            };
            let options = RenderOptions::from_cli(format, repo).unwrap();

            let output = render(review.into(), options).unwrap();

            assert!(output.contains("security"));
            assert!(output.contains("quality"));
            assert!(output.contains("Total token usage"));
            assert!(output.contains(expected_usage));
            assert!(output.contains("Review summary"));
            assert!(output.contains("Peer version:"));
            assert!(output.contains(expected_provider));
            assert!(
                output.find("Review summary").unwrap() < output.find("Total token usage").unwrap()
            );
            assert!(output.find("Total token usage").unwrap() < output.find("security").unwrap());
        }
    }

    #[test]
    fn check_document_moves_shared_metadata_to_the_envelope() {
        let mut check = result();
        check.context_usage = Some(review_context_usage());
        let ordered_commits = check.ordered_commits.clone();

        let document = RenderDocument::from(check);

        assert!(document.summary.is_none());
        assert_eq!(document.context_usage, Some(review_context_usage()));
        assert_eq!(document.ordered_commits, ordered_commits);
        assert_eq!(document.checks.len(), 1);
        assert_matches!(
            &document.checks[0].outcome,
            RenderCheckOutcome::Issues { .. }
        );
    }

    #[test]
    fn render_documents_round_trip_through_json() {
        let document = RenderDocument::from(result());

        let encoded = serde_json::to_string(&document).unwrap();
        let decoded: RenderDocument = serde_json::from_str(&encoded).unwrap();
        let value: serde_json::Value = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, document);
        assert!(value.get("summary").is_none());
        assert!(value.get("context_usage").is_none());
    }

    #[test]
    fn omits_empty_metadata_for_execution_failures() {
        let check = RenderCheck {
            check: "quality".into(),
            target: CheckTarget::Commit(CommitHash::new("abc1234").unwrap()),
            outcome: RenderCheckOutcome::Failed {
                failure: RenderCheckFailure::Execution {
                    reason: "provider unavailable".into(),
                },
            },
        };

        assert!(!markdown::render(&check).contains("### Metadata"));
        assert!(!github::render(&check, "owner/repo").contains("### Metadata"));
    }

    #[test]
    fn check_results_convert_to_exclusive_outcomes() {
        let mut clean = result();
        clean.findings.clear();
        assert_matches!(
            &RenderDocument::from(clean).checks[0].outcome,
            RenderCheckOutcome::Clean { .. }
        );

        assert_matches!(
            &RenderDocument::from(result()).checks[0].outcome,
            RenderCheckOutcome::Issues { .. }
        );

        let mut exhausted = result();
        exhausted.error = Some(CheckError::Exhausted {
            reason: "iteration limit reached".into(),
        });
        assert_matches!(
            &RenderDocument::from(exhausted).checks[0].outcome,
            RenderCheckOutcome::Exhausted { .. }
        );

        let mut failed = result();
        failed.error = Some(CheckError::ClarificationRequired {
            questions: vec!["Which policy applies?".into()],
        });
        assert_matches!(
            &RenderDocument::from(failed).checks[0].outcome,
            RenderCheckOutcome::Failed {
                failure: RenderCheckFailure::Check { .. }
            }
        );
    }

    #[test]
    fn review_checks_are_combined_and_sorted_by_check_then_commit() {
        let first = CommitHash::new("abc1234").unwrap();
        let second = CommitHash::new("def5678").unwrap();
        let mut security_second = result();
        security_second.target = CheckTarget::Commit(second.clone());
        let mut intent_second = result();
        intent_second.check = "intent".into();
        intent_second.target = CheckTarget::Commit(second.clone());
        let mut security_first = result();
        security_first.target = CheckTarget::Commit(first.clone());
        let review = crate::review::ReviewResult {
            summary: review_summary(),
            context_usage: None,
            ordered_commits: vec![first.clone(), second],
            checks: vec![security_second, intent_second, security_first],
            errors: vec![crate::review::ReviewCheckError {
                check: ReviewCheck::Quality {
                    revision: first.clone(),
                },
                error: crate::check::CheckCommandError::Config(
                    crate::error::PeerError::invalid_config("missing api key"),
                ),
            }],
        };

        let document = RenderDocument::from(review);

        assert_eq!(
            document
                .checks
                .iter()
                .map(|check| (check.check.as_str(), check.target.to_string()))
                .collect::<Vec<_>>(),
            [
                ("intent", "def5678".to_string()),
                ("quality", "abc1234".to_string()),
                ("security", "abc1234".to_string()),
                ("security", "def5678".to_string()),
            ]
        );
        assert_matches!(
            &document.checks[1].outcome,
            RenderCheckOutcome::Failed {
                failure: RenderCheckFailure::Execution { reason }
            } if reason == "missing api key"
        );
    }

    #[test]
    fn github_requires_a_repo() {
        assert_eq!(
            RenderOptions::from_cli(OutputFormat::Github, None),
            Err(RenderOptionsError::GithubRepoRequired)
        );
    }
}
