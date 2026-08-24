mod github;
mod markdown;
mod terminal;

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::cli::OutputFormat;
use crate::git::CommitHash;
use crate::llm::LlmUsage;
use crate::review::{
    PipelineExecutionError, PipelineReviewResult, PipelineStageResult, ReviewSummary,
};
use crate::stage::{
    ClarificationQuestion, FileLocation, Finding, KnowledgeQuestion, Severity, StageFailure,
    StageKind, StageOutcome, StageResult, StageRun, StageTarget, StructuralRecommendation,
};

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RenderDocument {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<ReviewSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_usage: Option<LlmUsage>,
    pub ordered_commits: Vec<CommitHash>,
    #[serde(default)]
    pub questions: Vec<KnowledgeQuestion>,
    #[serde(default)]
    pub recommendations: Vec<StructuralRecommendation>,
    #[serde(default)]
    pub findings: Vec<RenderFinding>,
    pub stages: Vec<RenderStage>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RenderFinding {
    pub commit: CommitHash,
    pub severity: Severity,
    pub message: String,
    #[serde(flatten)]
    pub location: Option<FileLocation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security: Option<RenderSecurityContext>,
}

/// A JSON value accepted by the `render` command.
///
/// Individual review items are promoted to a minimal render document so they
/// use the same formatting and escaping rules as a complete review.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum RenderInput {
    Document(RenderDocument),
    KnowledgeQuestion(KnowledgeQuestion),
    StructuralRecommendation(StructuralRecommendation),
    Finding(RenderFinding),
}

impl From<RenderDocument> for RenderInput {
    fn from(document: RenderDocument) -> Self {
        Self::Document(document)
    }
}

impl From<PipelineReviewResult> for RenderInput {
    fn from(review: PipelineReviewResult) -> Self {
        Self::Document(review.into())
    }
}

impl From<KnowledgeQuestion> for RenderInput {
    fn from(question: KnowledgeQuestion) -> Self {
        Self::KnowledgeQuestion(question)
    }
}

impl From<StructuralRecommendation> for RenderInput {
    fn from(recommendation: StructuralRecommendation) -> Self {
        Self::StructuralRecommendation(recommendation)
    }
}

impl From<RenderFinding> for RenderInput {
    fn from(finding: RenderFinding) -> Self {
        Self::Finding(finding)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RenderSecurityContext {
    pub attacker_control: String,
    pub sensitive_operation: String,
    pub impact: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct RenderStage {
    pub stage: String,
    pub target: StageTarget,
    pub outcome: RenderStageOutcome,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RenderStageOutcome {
    Clean {
        summary: String,
        iterations: u32,
        usage: LlmUsage,
    },
    Issues {
        summary: String,
        iterations: u32,
        usage: LlmUsage,
    },
    Exhausted {
        summary: String,
        reason: String,
        iterations: u32,
        usage: LlmUsage,
    },
    Failed {
        failure: RenderStageFailure,
    },
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RenderStageFailure {
    Stage {
        summary: String,
        failure: StageFailure,
        iterations: u32,
        usage: LlmUsage,
    },
    Execution {
        reason: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<LlmUsage>,
    },
}

enum RenderStageErrorRef<'a> {
    Exhausted(&'a str),
    Stage(&'a StageFailure),
    Execution(&'a str),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ReviewCounts {
    info: usize,
    low: usize,
    medium: usize,
    high: usize,
    critical: usize,
    exhausted: usize,
    failed: usize,
}

impl RenderStage {
    fn status(&self) -> &'static str {
        match self.outcome {
            RenderStageOutcome::Clean { .. } => "clean",
            RenderStageOutcome::Issues { .. } => "issues",
            RenderStageOutcome::Exhausted { .. } => "exhausted",
            RenderStageOutcome::Failed { .. } => "failed",
        }
    }

    fn summary(&self) -> Option<&str> {
        match &self.outcome {
            RenderStageOutcome::Clean { summary, .. } => Some(summary),
            RenderStageOutcome::Issues { summary, .. } => Some(summary),
            RenderStageOutcome::Exhausted { summary, .. } => Some(summary),
            RenderStageOutcome::Failed {
                failure: RenderStageFailure::Stage { summary, .. },
            } => Some(summary),
            RenderStageOutcome::Failed {
                failure: RenderStageFailure::Execution { .. },
            } => None,
        }
    }

    fn iterations(&self) -> Option<u32> {
        match &self.outcome {
            RenderStageOutcome::Clean { iterations, .. } => Some(*iterations),
            RenderStageOutcome::Issues { iterations, .. } => Some(*iterations),
            RenderStageOutcome::Exhausted { iterations, .. } => Some(*iterations),
            RenderStageOutcome::Failed {
                failure: RenderStageFailure::Stage { iterations, .. },
            } => Some(*iterations),
            RenderStageOutcome::Failed {
                failure: RenderStageFailure::Execution { .. },
            } => None,
        }
    }

    fn usage(&self) -> Option<&LlmUsage> {
        match &self.outcome {
            RenderStageOutcome::Clean { usage, .. } => Some(usage),
            RenderStageOutcome::Issues { usage, .. } => Some(usage),
            RenderStageOutcome::Exhausted { usage, .. } => Some(usage),
            RenderStageOutcome::Failed {
                failure: RenderStageFailure::Stage { usage, .. },
            } => Some(usage),
            RenderStageOutcome::Failed {
                failure: RenderStageFailure::Execution { usage, .. },
            } => usage.as_ref(),
        }
    }

    fn error(&self) -> Option<RenderStageErrorRef<'_>> {
        match &self.outcome {
            RenderStageOutcome::Exhausted { reason, .. } => {
                Some(RenderStageErrorRef::Exhausted(reason))
            }
            RenderStageOutcome::Failed {
                failure: RenderStageFailure::Stage { failure, .. },
            } => Some(RenderStageErrorRef::Stage(failure)),
            RenderStageOutcome::Failed {
                failure: RenderStageFailure::Execution { reason, .. },
            } => Some(RenderStageErrorRef::Execution(reason)),
            RenderStageOutcome::Clean { .. } => None,
            RenderStageOutcome::Issues { .. } => None,
        }
    }
}

impl From<StageResult> for RenderDocument {
    fn from(mut result: StageResult) -> Self {
        let context_usage = result.context_usage.take();
        let ordered_commits = result.ordered_commits.clone();
        let RenderStageParts {
            stage,
            questions,
            recommendations,
            findings,
        } = result.into();
        Self {
            summary: None,
            context_usage,
            ordered_commits,
            questions,
            recommendations,
            findings,
            stages: vec![stage],
        }
    }
}

impl From<PipelineReviewResult> for RenderDocument {
    fn from(review: PipelineReviewResult) -> Self {
        let PipelineReviewResult {
            summary,
            ordered_commits,
            stages,
            errors,
        } = review;
        let stage_parts = stages
            .into_iter()
            .map(RenderStageParts::from)
            .chain(errors.into_iter().map(RenderStageParts::from))
            .collect::<Vec<_>>();
        let (stages, questions, recommendations, findings) =
            aggregate(stage_parts, &ordered_commits);
        Self {
            summary: Some(summary),
            context_usage: None,
            ordered_commits,
            questions,
            recommendations,
            findings,
            stages,
        }
    }
}

struct RenderStageParts {
    stage: RenderStage,
    questions: Vec<KnowledgeQuestion>,
    recommendations: Vec<StructuralRecommendation>,
    findings: Vec<RenderFinding>,
}

fn aggregate(
    mut parts: Vec<RenderStageParts>,
    ordered_commits: &[CommitHash],
) -> (
    Vec<RenderStage>,
    Vec<KnowledgeQuestion>,
    Vec<StructuralRecommendation>,
    Vec<RenderFinding>,
) {
    parts.sort_by_key(|parts| render_stage_order(&parts.stage, ordered_commits));
    let mut stages = Vec::with_capacity(parts.len());
    let mut questions = Vec::new();
    let mut recommendations = Vec::new();
    let mut findings = Vec::new();
    for part in parts {
        stages.push(part.stage);
        questions.extend(part.questions);
        recommendations.extend(part.recommendations);
        findings.extend(part.findings);
    }
    sort_by_related_commits(&mut questions, ordered_commits, |question| {
        &question.related_commits
    });
    sort_by_related_commits(&mut recommendations, ordered_commits, |recommendation| {
        &recommendation.related_commits
    });
    sort_render_findings(&mut findings, ordered_commits);
    (stages, questions, recommendations, findings)
}

#[derive(Default)]
struct RenderStageOutput {
    questions: Vec<KnowledgeQuestion>,
    recommendations: Vec<StructuralRecommendation>,
    findings: Vec<RenderFinding>,
}

impl RenderStageOutput {
    fn has_issues(&self) -> bool {
        !self.questions.is_empty() || !self.recommendations.is_empty() || !self.findings.is_empty()
    }
}

impl From<PipelineStageResult> for RenderStageParts {
    fn from(stage: PipelineStageResult) -> Self {
        match stage {
            PipelineStageResult::ReviewContext(run) => {
                render_typed_run(run, |report| (report.summary, RenderStageOutput::default()))
            }
            PipelineStageResult::Knowledge(run) => render_knowledge_run(run),
            PipelineStageResult::Quality(run) => render_typed_run(run, |report| {
                let findings = report.findings.into_iter().map(render_finding).collect();
                (
                    report.summary,
                    RenderStageOutput {
                        findings,
                        ..RenderStageOutput::default()
                    },
                )
            }),
            PipelineStageResult::Security(run) => render_typed_run(run, |report| {
                let findings = report
                    .findings
                    .into_iter()
                    .map(|finding| RenderFinding {
                        commit: finding.commit,
                        severity: finding.severity,
                        message: finding.message,
                        location: finding.location,
                        security: Some(RenderSecurityContext {
                            attacker_control: finding.attacker_control,
                            sensitive_operation: finding.sensitive_operation,
                            impact: finding.impact,
                        }),
                    })
                    .collect();
                (
                    report.summary,
                    RenderStageOutput {
                        findings,
                        ..RenderStageOutput::default()
                    },
                )
            }),
        }
    }
}

impl From<PipelineExecutionError> for RenderStageParts {
    fn from(error: PipelineExecutionError) -> Self {
        Self {
            stage: RenderStage {
                stage: error.stage.as_str().to_string(),
                target: error.target,
                outcome: RenderStageOutcome::Failed {
                    failure: RenderStageFailure::Execution {
                        reason: error.reason,
                        usage: error.usage,
                    },
                },
            },
            questions: Vec::new(),
            recommendations: Vec::new(),
            findings: Vec::new(),
        }
    }
}

fn render_knowledge_run(run: StageRun<crate::stage::KnowledgeReport>) -> RenderStageParts {
    render_typed_run(run, |report| {
        (
            report.summary,
            RenderStageOutput {
                questions: report.questions,
                recommendations: report.recommendations,
                findings: Vec::new(),
            },
        )
    })
}

fn render_typed_run<R>(
    run: StageRun<R>,
    completed: impl FnOnce(R) -> (String, RenderStageOutput),
) -> RenderStageParts {
    let StageRun {
        stage,
        target,
        ordered_commits,
        outcome,
        iterations,
        usage,
    } = run;
    let (outcome, output) = match outcome {
        StageOutcome::Completed { report } => {
            let (summary, mut output) = completed(report);
            sort_by_related_commits(&mut output.questions, &ordered_commits, |question| {
                &question.related_commits
            });
            sort_by_related_commits(
                &mut output.recommendations,
                &ordered_commits,
                |recommendation| &recommendation.related_commits,
            );
            sort_render_findings(&mut output.findings, &ordered_commits);
            if output.has_issues() {
                (
                    RenderStageOutcome::Issues {
                        summary,
                        iterations,
                        usage,
                    },
                    output,
                )
            } else {
                (
                    RenderStageOutcome::Clean {
                        summary,
                        iterations,
                        usage,
                    },
                    output,
                )
            }
        }
        StageOutcome::Blocked { questions } => (
            RenderStageOutcome::Failed {
                failure: RenderStageFailure::Stage {
                    summary: "Additional review information is required.".to_string(),
                    failure: StageFailure::ClarificationRequired { questions },
                    iterations,
                    usage,
                },
            },
            RenderStageOutput::default(),
        ),
        StageOutcome::Exhausted { reason } => (
            RenderStageOutcome::Exhausted {
                summary: "Stage did not complete.".to_string(),
                reason,
                iterations,
                usage,
            },
            RenderStageOutput::default(),
        ),
    };
    RenderStageParts {
        stage: RenderStage {
            stage: stage.as_str().to_string(),
            target,
            outcome,
        },
        questions: output.questions,
        recommendations: output.recommendations,
        findings: output.findings,
    }
}

impl From<StageResult> for RenderStageParts {
    fn from(result: StageResult) -> Self {
        let StageResult {
            stage,
            target,
            ordered_commits,
            summary,
            mut findings,
            iterations,
            failure,
            context_usage: _,
            usage,
        } = result;
        sort_findings_by_commit(&mut findings, &ordered_commits);
        let findings = findings.into_iter().map(render_finding).collect::<Vec<_>>();
        let outcome = match failure {
            None if findings.is_empty() => RenderStageOutcome::Clean {
                summary,
                iterations,
                usage,
            },
            None => RenderStageOutcome::Issues {
                summary,
                iterations,
                usage,
            },
            Some(StageFailure::Exhausted { reason }) => RenderStageOutcome::Exhausted {
                summary,
                reason,
                iterations,
                usage,
            },
            Some(failure) => RenderStageOutcome::Failed {
                failure: RenderStageFailure::Stage {
                    summary,
                    failure,
                    iterations,
                    usage,
                },
            },
        };
        Self {
            stage: RenderStage {
                stage,
                target,
                outcome,
            },
            questions: Vec::new(),
            recommendations: Vec::new(),
            findings,
        }
    }
}

fn sort_findings_by_commit(findings: &mut [Finding], ordered_commits: &[CommitHash]) {
    findings.sort_by_cached_key(|finding| {
        ordered_commits
            .iter()
            .position(|commit| commit.matches(&finding.commit))
            .unwrap_or(usize::MAX)
    });
}

fn render_finding(finding: Finding) -> RenderFinding {
    RenderFinding {
        commit: finding.commit,
        severity: finding.severity,
        message: finding.message,
        location: finding.location,
        security: None,
    }
}

fn sort_by_related_commits<T>(
    items: &mut [T],
    ordered_commits: &[CommitHash],
    commits: impl Fn(&T) -> &[CommitHash],
) {
    items.sort_by_cached_key(|item| {
        commits(item)
            .iter()
            .filter_map(|commit| {
                ordered_commits
                    .iter()
                    .position(|ordered| ordered.matches(commit))
            })
            .min()
            .unwrap_or(usize::MAX)
    });
}

fn sort_render_findings(findings: &mut [RenderFinding], ordered_commits: &[CommitHash]) {
    findings.sort_by_cached_key(|finding| {
        ordered_commits
            .iter()
            .position(|commit| commit.matches(&finding.commit))
            .unwrap_or(usize::MAX)
    });
}

fn render_stage_order(stage: &RenderStage, ordered_commits: &[CommitHash]) -> (usize, usize) {
    fn stage_rank(stage: StageKind) -> usize {
        match stage {
            StageKind::ReviewContext => 0,
            StageKind::Knowledge => 100,
            StageKind::Quality => 200,
            StageKind::Security => 300,
        }
    }

    let stage_order = stage
        .stage
        .parse::<StageKind>()
        .ok()
        .map(stage_rank)
        .unwrap_or(usize::MAX);
    let commit_order = match &stage.target {
        StageTarget::Commit(target) => ordered_commits
            .iter()
            .position(|commit| commit.matches(target))
            .unwrap_or(usize::MAX),
        StageTarget::Range { .. } => usize::MAX,
    };
    (stage_order, commit_order)
}

fn clarification_message(questions: &[ClarificationQuestion]) -> String {
    questions
        .iter()
        .map(|question| format!("{} ({})", question.question, question.reason))
        .collect::<Vec<_>>()
        .join("; ")
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

pub fn render(input: RenderInput, options: RenderOptions) -> Result<String, RenderError> {
    match options.format {
        RenderFormat::Terminal => Ok(terminal::render(&input)),
        RenderFormat::Markdown => Ok(markdown::render(&input)),
        RenderFormat::Github { repo } => Ok(github::render(&input, &repo)),
    }
}

pub fn render_pipeline_json(review: PipelineReviewResult) -> Result<String, RenderError> {
    render_json(review.into())
}

fn render_json(document: RenderDocument) -> Result<String, RenderError> {
    Ok(serde_json::to_string_pretty(&document)?)
}

fn review_counts(document: &RenderDocument) -> ReviewCounts {
    let mut counts = ReviewCounts::default();
    for severity in document.findings.iter().map(|finding| finding.severity) {
        match severity {
            Severity::Info => counts.info += 1,
            Severity::Low => counts.low += 1,
            Severity::Medium => counts.medium += 1,
            Severity::High => counts.high += 1,
            Severity::Critical => counts.critical += 1,
        }
    }
    for stage in &document.stages {
        match stage.outcome {
            RenderStageOutcome::Exhausted { .. } => counts.exhausted += 1,
            RenderStageOutcome::Failed { .. } => counts.failed += 1,
            RenderStageOutcome::Clean { .. } => {}
            RenderStageOutcome::Issues { .. } => {}
        }
    }
    counts
}

fn usage_by_model(document: &RenderDocument) -> BTreeMap<String, crate::review::ModelUsage> {
    let mut usage = BTreeMap::new();
    for item in document
        .context_usage
        .iter()
        .chain(document.stages.iter().filter_map(RenderStage::usage))
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

impl From<serde_json::Error> for RenderError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialization(error)
    }
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
    use super::*;

    use std::assert_matches;

    use crate::stage::FileLocation;

    fn result() -> StageResult {
        StageResult {
            stage: "security".to_string(),
            target: StageTarget::Range {
                from: CommitHash::new("abc1234").unwrap(),
                to: CommitHash::new("def5678").unwrap(),
            },
            ordered_commits: vec![
                CommitHash::new("abc1234").unwrap(),
                CommitHash::new("def5678").unwrap(),
            ],
            summary: "Reviewed the change.".to_string(),
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
            failure: None,
            context_usage: None,
            usage: LlmUsage {
                input_tokens: 100,
                output_tokens: 20,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: 0.001,
                model: "test-model".to_string(),
                models: Vec::new(),
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
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_usd: 0.0004,
            model: "test-model".to_string(),
            models: Vec::new(),
        }
    }

    fn review_ordered_commits() -> Vec<CommitHash> {
        vec![
            CommitHash::new("abc1234").unwrap(),
            CommitHash::new("def5678").unwrap(),
        ]
    }

    fn exhausted_stage_run<R>(
        stage: crate::stage::StageKind,
        target: StageTarget,
        ordered_commits: &[CommitHash],
    ) -> StageRun<R> {
        StageRun {
            stage,
            target,
            ordered_commits: ordered_commits.to_vec(),
            outcome: StageOutcome::Exhausted {
                reason: "iteration limit reached".to_string(),
            },
            iterations: 3,
            usage: result().usage,
        }
    }

    fn pipeline_error(
        stage: crate::stage::StageKind,
        target: StageTarget,
    ) -> PipelineExecutionError {
        PipelineExecutionError {
            stage,
            target,
            reason: "stage execution failed".to_string(),
            usage: None,
        }
    }

    #[test]
    fn pipeline_execution_failure_usage_is_included_in_totals() {
        let usage = result().usage;
        let expected = usage.clone();
        let review = PipelineReviewResult {
            summary: review_summary(),
            ordered_commits: review_ordered_commits(),
            stages: Vec::new(),
            errors: vec![PipelineExecutionError {
                stage: crate::stage::StageKind::Quality,
                target: StageTarget::Commit(CommitHash::new("abc1234").unwrap()),
                reason: "invalid typed stage report".to_string(),
                usage: Some(usage),
            }],
        };

        let document = RenderDocument::from(review);
        let totals = usage_by_model(&document);

        assert_eq!(document.stages[0].usage(), Some(&expected));
        assert_eq!(totals[&expected.model].input_tokens, expected.input_tokens);
        assert_eq!(
            totals[&expected.model].output_tokens,
            expected.output_tokens
        );
        assert_eq!(totals[&expected.model].cost_usd, expected.cost_usd);
    }

    fn review_document(
        stages: Vec<StageResult>,
        context_usage: Option<LlmUsage>,
    ) -> RenderDocument {
        let ordered_commits = review_ordered_commits();
        let stage_parts = stages
            .into_iter()
            .map(RenderStageParts::from)
            .collect::<Vec<_>>();
        let (stages, questions, recommendations, findings) =
            aggregate(stage_parts, &ordered_commits);
        RenderDocument {
            summary: Some(review_summary()),
            context_usage,
            ordered_commits,
            questions,
            recommendations,
            findings,
            stages,
        }
    }

    #[test]
    fn orders_findings_with_abbreviated_commit_hashes() {
        let mut result = result();
        result.ordered_commits = vec![
            CommitHash::new(&format!("abc1234{}", "0".repeat(33))).unwrap(),
            CommitHash::new(&format!("def5678{}", "0".repeat(33))).unwrap(),
        ];

        let result = RenderStageParts::from(result);
        assert_matches!(
            &result.findings[0],
            RenderFinding { commit, .. } if commit.as_ref() == "abc1234"
        );
        assert_matches!(
            &result.findings[1],
            RenderFinding { commit, .. } if commit.as_ref() == "def5678"
        );
    }

    #[test]
    fn knowledge_questions_and_recommendations_are_stored_and_rendered_separately() {
        let commit = CommitHash::new("abc1234").unwrap();
        let report: crate::stage::KnowledgeReport = serde_json::from_value(serde_json::json!({
            "summary": "One question and one recommendation.",
            "questions": [{
                "category": "rationale",
                "question": "Why is the retry limit three?",
                "evidence": "The diff introduces a literal retry limit.",
                "why_it_matters": "Future tuning needs the original constraint.",
                "related_commits": [commit]
            }],
            "recommendations": [{
                "kind": "split_commit",
                "message": "Split the migration from the retry change.",
                "rationale": "The migration is independently reviewable.",
                "related_commits": [commit]
            }]
        }))
        .unwrap();
        let review = PipelineReviewResult {
            summary: review_summary(),
            ordered_commits: vec![commit.clone()],
            stages: vec![PipelineStageResult::Knowledge(StageRun {
                stage: StageKind::Knowledge,
                target: StageTarget::Commit(commit.clone()),
                ordered_commits: vec![commit],
                outcome: StageOutcome::Completed { report },
                iterations: 1,
                usage: LlmUsage::zero("test-model"),
            })],
            errors: Vec::new(),
        };

        let document = RenderDocument::from(review);

        assert_eq!(document.questions.len(), 1);
        assert_eq!(document.recommendations.len(), 1);
        assert!(document.findings.is_empty());
        assert_matches!(
            &document.stages[0].outcome,
            RenderStageOutcome::Issues { .. }
        );
        assert_matches!(
            &document.questions[0],
            crate::stage::KnowledgeQuestion { question, .. }
                if question == "Why is the retry limit three?"
        );
        assert_matches!(
            &document.recommendations[0],
            crate::stage::StructuralRecommendation { message, .. }
                if message == "Split the migration from the retry change."
        );
        let output = render(
            document.into(),
            RenderOptions::from_cli(OutputFormat::Markdown, None).unwrap(),
        )
        .unwrap();
        assert_eq!(output.matches("## Review questions").count(), 1);
        assert_eq!(output.matches("## Structural recommendations").count(), 1);
        assert!(!output.contains("## Review findings"));
        assert!(output.contains("**question/rationale**"));
        assert!(output.contains("**recommendation/split_commit**"));
    }

    #[test]
    fn pipeline_outputs_preserve_separate_stage_data_without_duplicating_knowledge_items() {
        let first = CommitHash::new("abc1234").unwrap();
        let second = CommitHash::new("def5678").unwrap();
        let ordered_commits = vec![first.clone(), second.clone()];
        let range = StageTarget::Range {
            from: first.clone(),
            to: second.clone(),
        };
        let knowledge: crate::stage::KnowledgeReport = serde_json::from_value(serde_json::json!({
            "summary": "Knowledge feedback found.",
            "questions": [{
                "category": "rationale",
                "question": "Why is the retry limit three?",
                "evidence": "The diff introduces a literal limit.",
                "why_it_matters": "Future tuning needs the constraint.",
                "related_commits": [first, second],
                "location": {
                    "commit": second,
                    "file": "src/retry.rs",
                    "line": 12
                }
            }],
            "recommendations": [{
                "kind": "split_commit",
                "message": "Split the migration from the retry change.",
                "rationale": "They are independently reviewable.",
                "related_commits": [first, second]
            }]
        }))
        .unwrap();
        let quality: crate::stage::QualityReport = serde_json::from_value(serde_json::json!({
            "summary": "One quality issue.",
            "findings": [{
                "commit": first,
                "severity": "low",
                "message": "The fallback is unreachable.",
                "file": "src/main.rs",
                "line": 7
            }]
        }))
        .unwrap();
        let security: crate::stage::SecurityReport = serde_json::from_value(serde_json::json!({
            "summary": "One security issue.",
            "findings": [{
                "commit": second,
                "severity": "high",
                "message": "Untrusted input reaches a shell.",
                "attacker_control": "A request parameter controls the command.",
                "sensitive_operation": "The parameter is passed to a shell.",
                "impact": "Arbitrary command execution.",
                "file": "src/command.rs",
                "line": 42
            }]
        }))
        .unwrap();
        let usage = LlmUsage::zero("test-model");
        let review = PipelineReviewResult {
            summary: review_summary(),
            ordered_commits: ordered_commits.clone(),
            stages: vec![
                PipelineStageResult::Knowledge(StageRun {
                    stage: StageKind::Knowledge,
                    target: range,
                    ordered_commits: ordered_commits.clone(),
                    outcome: StageOutcome::Completed { report: knowledge },
                    iterations: 1,
                    usage: usage.clone(),
                }),
                PipelineStageResult::Quality(StageRun {
                    stage: StageKind::Quality,
                    target: StageTarget::Commit(first),
                    ordered_commits: ordered_commits.clone(),
                    outcome: StageOutcome::Completed { report: quality },
                    iterations: 1,
                    usage: usage.clone(),
                }),
                PipelineStageResult::Security(StageRun {
                    stage: StageKind::Security,
                    target: StageTarget::Commit(second),
                    ordered_commits,
                    outcome: StageOutcome::Completed { report: security },
                    iterations: 1,
                    usage,
                }),
            ],
            errors: Vec::new(),
        };

        let document = RenderDocument::from(review);

        assert_eq!(document.questions.len(), 1);
        assert_eq!(document.recommendations.len(), 1);
        assert_eq!(document.findings.len(), 2);
        assert_eq!(
            review_counts(&document),
            ReviewCounts {
                low: 1,
                high: 1,
                ..ReviewCounts::default()
            }
        );

        let json = render_json(document.clone()).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value.get("feedback"), None);
        assert_eq!(
            value["questions"][0]["related_commits"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(value["recommendations"].as_array().unwrap().len(), 1);
        assert_eq!(value["findings"].as_array().unwrap().len(), 2);
        assert_eq!(value["findings"][0].get("security"), None);
        assert_eq!(
            value["findings"][1]["security"]["attacker_control"],
            "A request parameter controls the command."
        );
        assert_eq!(
            value["findings"][1]["security"]["sensitive_operation"],
            "The parameter is passed to a shell."
        );
        assert_eq!(
            value["findings"][1]["security"]["impact"],
            "Arbitrary command execution."
        );

        for (format, repo) in [
            (OutputFormat::Terminal, None),
            (OutputFormat::Markdown, None),
            (OutputFormat::Github, Some("owner/repo".to_string())),
        ] {
            let output = render(
                document.clone().into(),
                RenderOptions::from_cli(format, repo).unwrap(),
            )
            .unwrap();
            assert_eq!(output.matches("question/rationale").count(), 1);
            assert_eq!(output.matches("recommendation/split_commit").count(), 1);
            assert!(output.contains("finding/"));
            assert!(output.contains("high"));
            assert!(output.contains("Attacker control:"));
            assert!(output.contains("Sensitive operation:"));
            assert!(output.contains("Impact:"));
            assert!(output.contains("Review questions"));
            assert!(output.contains("Structural recommendations"));
            assert!(output.contains("Review findings"));
        }
    }

    #[test]
    fn rejects_unified_render_documents_with_feedback() {
        let error = serde_json::from_value::<RenderDocument>(serde_json::json!({
            "ordered_commits": [],
            "feedback": [],
            "stages": []
        }))
        .unwrap_err();

        assert!(error.to_string().contains("unknown field `feedback`"));
    }

    #[test]
    fn pipeline_stage_outcomes_convert_to_render_outcomes() {
        let commit = CommitHash::new("abc1234").unwrap();
        let target = StageTarget::Commit(commit.clone());
        let ordered_commits = vec![commit];
        let clean = PipelineStageResult::ReviewContext(StageRun {
            stage: crate::stage::StageKind::ReviewContext,
            target: target.clone(),
            ordered_commits: ordered_commits.clone(),
            outcome: StageOutcome::Completed {
                report: crate::stage::ReviewContextReport {
                    summary: "Context is sufficient.".to_string(),
                    objectives: Vec::new(),
                    expected_behavior: Vec::new(),
                    scope: Vec::new(),
                    constraints: Vec::new(),
                    implementation: Vec::new(),
                    verification: Vec::new(),
                    unresolved: Vec::new(),
                },
            },
            iterations: 1,
            usage: result().usage,
        });
        let blocked_outcome = serde_json::from_value::<
            StageOutcome<crate::stage::ReviewContextReport>,
        >(serde_json::json!({
            "status": "blocked",
            "questions": [{
                "question": "Which policy applies?",
                "reason": "The requirements conflict."
            }]
        }))
        .unwrap();
        let blocked = PipelineStageResult::ReviewContext(StageRun {
            stage: crate::stage::StageKind::ReviewContext,
            target: target.clone(),
            ordered_commits: ordered_commits.clone(),
            outcome: blocked_outcome,
            iterations: 2,
            usage: result().usage,
        });
        let exhausted = PipelineStageResult::ReviewContext(exhausted_stage_run(
            crate::stage::StageKind::ReviewContext,
            target,
            &ordered_commits,
        ));

        assert_matches!(
            RenderStageParts::from(clean).stage.outcome,
            RenderStageOutcome::Clean { summary, .. } if summary == "Context is sufficient."
        );
        assert_matches!(
            RenderStageParts::from(blocked).stage.outcome,
            RenderStageOutcome::Failed {
                failure: RenderStageFailure::Stage {
                    failure: StageFailure::ClarificationRequired { questions },
                    ..
                }
            } if questions == [ClarificationQuestion {
                question: "Which policy applies?".to_string(),
                reason: "The requirements conflict.".to_string(),
            }]
        );
        assert_matches!(
            RenderStageParts::from(exhausted).stage.outcome,
            RenderStageOutcome::Exhausted { reason, .. }
                if reason == "iteration limit reached"
        );
    }

    #[test]
    fn pipeline_stages_and_errors_sort_by_stage_then_commit() {
        use crate::stage::StageKind;

        let first = CommitHash::new("abc1234").unwrap();
        let second = CommitHash::new("def5678").unwrap();
        let ordered_commits = vec![first.clone(), second.clone()];
        let range = StageTarget::Range {
            from: first.clone(),
            to: second.clone(),
        };
        let review = PipelineReviewResult {
            summary: review_summary(),
            ordered_commits: ordered_commits.clone(),
            stages: vec![PipelineStageResult::ReviewContext(exhausted_stage_run(
                StageKind::ReviewContext,
                range.clone(),
                &ordered_commits,
            ))],
            errors: vec![
                pipeline_error(StageKind::Security, StageTarget::Commit(second.clone())),
                pipeline_error(StageKind::Knowledge, range.clone()),
                pipeline_error(StageKind::Quality, StageTarget::Commit(first.clone())),
                pipeline_error(StageKind::Security, StageTarget::Commit(first.clone())),
            ],
        };

        let document = RenderDocument::from(review);

        assert_eq!(
            document
                .stages
                .iter()
                .map(|stage| (stage.stage.as_str(), stage.target.to_string()))
                .collect::<Vec<_>>(),
            [
                ("review_context", "abc1234..def5678".to_string()),
                ("knowledge", "abc1234..def5678".to_string()),
                ("quality", "abc1234".to_string()),
                ("security", "abc1234".to_string()),
                ("security", "def5678".to_string()),
            ]
        );
    }

    #[test]
    fn render_orders_findings_by_commit_order() {
        let options = RenderOptions::from_cli(OutputFormat::Markdown, None).unwrap();
        let output = render(RenderDocument::from(result()).into(), options).unwrap();

        assert!(
            output.find(r"High\-risk finding\.").unwrap()
                < output.find(r"Informational finding\.").unwrap()
        );
    }

    #[test]
    fn serializes_review_results_as_ordered_stages() {
        let mut second = result();
        second.target = StageTarget::Commit(CommitHash::new("fedcba9").unwrap());
        let review = review_document(vec![result(), second], None);

        let output = render_json(review).unwrap();
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();

        assert!(output.starts_with("{\n  \"summary\""));
        assert_eq!(value["summary"]["peer_version"], "0.1.0");
        assert_eq!(value["summary"]["provider"], "test-provider");
        assert_eq!(value["summary"]["model"], "test-model");
        assert_eq!(value["summary"].get("usage_by_model"), None);
        assert_eq!(value["stages"].as_array().unwrap().len(), 2);
        assert_eq!(value["stages"][0]["stage"], "security");
        assert_eq!(value["questions"], serde_json::json!([]));
        assert_eq!(value["recommendations"], serde_json::json!([]));
        assert_eq!(value["findings"][0]["commit"], "abc1234");
        assert_eq!(value["stages"][1]["target"], "fedcba9");
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
            second.stage = "quality".into();
            let review = review_document(vec![result(), second], Some(review_context_usage()));
            let options = RenderOptions::from_cli(format, repo).unwrap();

            let output = render(review.into(), options).unwrap();

            assert_eq!(output.matches(expected_context_usage).count(), 1);
            assert!(output.contains(expected_total_usage));
        }
    }

    #[test]
    fn includes_review_context_usage_in_json_output() {
        let review = review_document(vec![result()], Some(review_context_usage()));

        let output = render_json(review).unwrap();
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();

        assert_eq!(value["context_usage"]["input_tokens"], 40);
        assert_eq!(value["context_usage"]["output_tokens"], 10);
        assert_eq!(value["context_usage"]["model"], "test-model");
        assert_eq!(value["stages"][0].get("context_usage"), None);
    }

    #[test]
    fn renders_review_summary_before_stages_in_human_readable_formats() {
        let mut second = result();
        second.stage = "quality".into();
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
            let review = review_document(vec![result(), second.clone()], None);
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
    fn stage_document_moves_shared_metadata_to_the_envelope() {
        let mut stage = result();
        stage.context_usage = Some(review_context_usage());
        let ordered_commits = stage.ordered_commits.clone();

        let document = RenderDocument::from(stage);

        assert_eq!(document.summary, None);
        assert_eq!(document.context_usage, Some(review_context_usage()));
        assert_eq!(document.ordered_commits, ordered_commits);
        assert_eq!(document.stages.len(), 1);
        assert_matches!(
            &document.stages[0].outcome,
            RenderStageOutcome::Issues { .. }
        );
    }

    #[test]
    fn render_documents_round_trip_through_json() {
        let document = RenderDocument::from(result());

        let encoded = serde_json::to_string(&document).unwrap();
        let decoded: RenderDocument = serde_json::from_str(&encoded).unwrap();
        let value: serde_json::Value = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, document);
        assert_eq!(value.get("summary"), None);
        assert_eq!(value.get("context_usage"), None);
    }

    #[test]
    fn render_input_deserializes_render_document() {
        let document = RenderDocument::from(result());
        let document_json = serde_json::to_string(&document).unwrap();

        let input = serde_json::from_str::<RenderInput>(&document_json).unwrap();

        assert_eq!(input, RenderInput::Document(document));
    }

    #[test]
    fn render_input_deserializes_knowledge_question() {
        let input = serde_json::from_str::<RenderInput>(
            r#"{"category":"rationale","question":"Why?","evidence":"The choice is not documented.","why_it_matters":"Future changes depend on it.","related_commits":["abc1234"]}"#,
        )
        .unwrap();

        assert_matches!(input, RenderInput::KnowledgeQuestion(question) if question.question == "Why?");
    }

    #[test]
    fn render_input_deserializes_structural_recommendation() {
        let input = serde_json::from_str::<RenderInput>(
            r#"{"kind":"split_commit","message":"Split this commit.","rationale":"The changes are independent.","related_commits":["abc1234"]}"#,
        )
        .unwrap();

        assert_matches!(
            input,
            RenderInput::StructuralRecommendation(recommendation)
                if recommendation.message == "Split this commit."
        );
    }

    #[test]
    fn render_input_deserializes_render_finding() {
        let input = serde_json::from_str::<RenderInput>(
            r#"{"commit":"abc1234","severity":"high","message":"Validate the input."}"#,
        )
        .unwrap();

        assert_matches!(input, RenderInput::Finding(finding) if finding.message == "Validate the input.");
    }

    #[test]
    fn individual_inputs_render_without_document_section_headings() {
        let inputs = [
            (
                serde_json::from_str::<RenderInput>(
                    r#"{"category":"rationale","question":"Why?","evidence":"Not documented.","why_it_matters":"Future changes depend on it.","related_commits":["abc1234"]}"#,
                )
                .unwrap(),
                "Why",
            ),
            (
                serde_json::from_str::<RenderInput>(
                    r#"{"kind":"split_commit","message":"Split this commit.","rationale":"The changes are independent.","related_commits":["abc1234"]}"#,
                )
                .unwrap(),
                "Split this commit",
            ),
            (
                serde_json::from_str::<RenderInput>(
                    r#"{"commit":"abc1234","severity":"high","message":"Validate the input."}"#,
                )
                .unwrap(),
                "Validate the input",
            ),
        ];

        for (input, expected) in inputs {
            for (format, repo) in [
                (OutputFormat::Terminal, None),
                (OutputFormat::Markdown, None),
                (OutputFormat::Github, Some("owner/repo".to_string())),
            ] {
                let output = render(
                    input.clone(),
                    RenderOptions::from_cli(format, repo).unwrap(),
                )
                .unwrap();

                assert!(output.contains(expected));
                assert!(!output.contains("Review questions"));
                assert!(!output.contains("Structural recommendations"));
                assert!(!output.contains("Review findings"));
            }
        }
    }

    #[test]
    fn execution_failures_deserialize_without_usage() {
        let failure: RenderStageFailure =
            serde_json::from_str(r#"{"type":"execution","reason":"provider unavailable"}"#)
                .unwrap();

        assert_eq!(
            failure,
            RenderStageFailure::Execution {
                reason: "provider unavailable".into(),
                usage: None,
            }
        );
    }

    #[test]
    fn counts_findings_and_incomplete_stages_for_review_summaries() {
        let mut exhausted = result();
        exhausted.findings[0].severity = Severity::Low;
        exhausted.findings[1].severity = Severity::Medium;
        exhausted.failure = Some(StageFailure::Exhausted {
            reason: "iteration limit reached".into(),
        });
        let mut failed = result();
        failed.findings.clear();
        failed.failure = Some(StageFailure::ClarificationRequired {
            questions: vec![ClarificationQuestion {
                question: "Which policy applies?".into(),
                reason: "The policy affects the finding.".into(),
            }],
        });
        let mut critical = result();
        critical.findings.truncate(1);
        critical.findings[0].severity = Severity::Critical;
        let document = review_document(vec![result(), exhausted, failed, critical], None);

        assert_eq!(
            review_counts(&document),
            ReviewCounts {
                info: 1,
                low: 1,
                medium: 1,
                high: 1,
                critical: 1,
                exhausted: 1,
                failed: 1,
            }
        );
    }

    #[test]
    fn renders_counts_in_human_readable_review_summaries() {
        for (format, repo, expected) in [
            (
                OutputFormat::Terminal,
                None,
                [
                    "- Info findings: 1",
                    "- High findings: 1",
                    "- Critical findings: 0",
                    "- Failed stages: 0",
                ],
            ),
            (
                OutputFormat::Markdown,
                None,
                [
                    "- **Info findings:** 1",
                    "- **High findings:** 1",
                    "- **Critical findings:** 0",
                    "- **Failed stages:** 0",
                ],
            ),
            (
                OutputFormat::Github,
                Some("owner/repo".to_string()),
                [
                    "- **Info findings:** 1",
                    "- **High findings:** 1",
                    "- **Critical findings:** 0",
                    "- **Failed stages:** 0",
                ],
            ),
        ] {
            let document = review_document(vec![result()], None);

            let output = render(
                document.into(),
                RenderOptions::from_cli(format, repo).unwrap(),
            )
            .unwrap();

            for line in expected {
                assert!(output.contains(line));
            }
        }
    }

    #[test]
    fn stage_results_convert_to_exclusive_outcomes() {
        let mut clean = result();
        clean.findings.clear();
        assert_matches!(
            &RenderDocument::from(clean).stages[0].outcome,
            RenderStageOutcome::Clean { .. }
        );

        assert_matches!(
            &RenderDocument::from(result()).stages[0].outcome,
            RenderStageOutcome::Issues { .. }
        );

        let mut exhausted = result();
        exhausted.failure = Some(StageFailure::Exhausted {
            reason: "iteration limit reached".into(),
        });
        assert_matches!(
            &RenderDocument::from(exhausted).stages[0].outcome,
            RenderStageOutcome::Exhausted { .. }
        );

        let mut failed = result();
        failed.failure = Some(StageFailure::ClarificationRequired {
            questions: vec![ClarificationQuestion {
                question: "Which policy applies?".into(),
                reason: "The policy affects the finding.".into(),
            }],
        });
        assert_matches!(
            &RenderDocument::from(failed).stages[0].outcome,
            RenderStageOutcome::Failed {
                failure: RenderStageFailure::Stage { .. }
            }
        );
    }

    #[test]
    fn review_stages_are_combined_and_sorted_by_stage_then_commit() {
        let first = CommitHash::new("abc1234").unwrap();
        let second = CommitHash::new("def5678").unwrap();
        let mut security_second = result();
        security_second.target = StageTarget::Commit(second.clone());
        let mut knowledge_second = result();
        knowledge_second.stage = "knowledge".into();
        knowledge_second.target = StageTarget::Commit(second.clone());
        let mut security_first = result();
        security_first.target = StageTarget::Commit(first.clone());
        let ordered_commits = vec![first.clone(), second];
        let mut stages = vec![security_second, knowledge_second, security_first]
            .into_iter()
            .map(|result| RenderStageParts::from(result).stage)
            .collect::<Vec<_>>();
        stages.push(RenderStage {
            stage: "quality".to_string(),
            target: StageTarget::Commit(first),
            outcome: RenderStageOutcome::Failed {
                failure: RenderStageFailure::Execution {
                    reason: "missing api key".to_string(),
                    usage: None,
                },
            },
        });
        stages.sort_by_key(|stage| render_stage_order(stage, &ordered_commits));
        let document = RenderDocument {
            summary: Some(review_summary()),
            context_usage: None,
            ordered_commits,
            questions: Vec::new(),
            recommendations: Vec::new(),
            findings: Vec::new(),
            stages,
        };

        assert_eq!(
            document
                .stages
                .iter()
                .map(|stage| (stage.stage.as_str(), stage.target.to_string()))
                .collect::<Vec<_>>(),
            [
                ("knowledge", "def5678".to_string()),
                ("quality", "abc1234".to_string()),
                ("security", "abc1234".to_string()),
                ("security", "def5678".to_string()),
            ]
        );
        assert_matches!(
            &document.stages[1].outcome,
            RenderStageOutcome::Failed {
                failure: RenderStageFailure::Execution { reason, .. }
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

    #[test]
    fn pipeline_json_preserves_typed_stage_envelope() {
        use crate::stage::StageKind;

        let first = CommitHash::new("abc1234").unwrap();
        let second = CommitHash::new("def5678").unwrap();
        let ordered_commits = vec![first.clone(), second.clone()];
        let range = StageTarget::Range {
            from: first.clone(),
            to: second.clone(),
        };
        let review = PipelineReviewResult {
            summary: review_summary(),
            ordered_commits: ordered_commits.clone(),
            stages: vec![
                PipelineStageResult::ReviewContext(exhausted_stage_run(
                    StageKind::ReviewContext,
                    range.clone(),
                    &ordered_commits,
                )),
                PipelineStageResult::Knowledge(exhausted_stage_run(
                    StageKind::Knowledge,
                    range,
                    &ordered_commits,
                )),
                PipelineStageResult::Quality(exhausted_stage_run(
                    StageKind::Quality,
                    StageTarget::Commit(second.clone()),
                    &ordered_commits,
                )),
                PipelineStageResult::Security(exhausted_stage_run(
                    StageKind::Security,
                    StageTarget::Commit(second),
                    &ordered_commits,
                )),
            ],
            errors: Vec::new(),
        };

        let output = render_pipeline_json(review).unwrap();
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        let stages = value["stages"].as_array().unwrap();

        assert_eq!(value["ordered_commits"][0], "abc1234");
        assert_eq!(value["questions"], serde_json::json!([]));
        assert_eq!(value["recommendations"], serde_json::json!([]));
        assert_eq!(value["findings"], serde_json::json!([]));
        assert_eq!(
            stages
                .iter()
                .map(|stage| stage["stage"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["review_context", "knowledge", "quality", "security"]
        );
        assert_eq!(stages.len(), 4);
        for stage in stages {
            assert_eq!(stage["outcome"]["status"], "exhausted");
            assert_eq!(stage["outcome"]["reason"], "iteration limit reached");
            assert_eq!(stage["outcome"]["iterations"], 3);
        }
    }

    #[test]
    fn pipeline_exhaustion_reason_appears_once_in_terminal() {
        let commit = CommitHash::new("abc1234").unwrap();
        let output = render(
            PipelineReviewResult {
                summary: review_summary(),
                ordered_commits: vec![commit.clone()],
                stages: vec![PipelineStageResult::ReviewContext(exhausted_stage_run(
                    crate::stage::StageKind::ReviewContext,
                    StageTarget::Commit(commit),
                    &[],
                ))],
                errors: Vec::new(),
            }
            .into(),
            RenderOptions::from_cli(OutputFormat::Terminal, None).unwrap(),
        )
        .unwrap();

        assert_eq!(output.matches("iteration limit reached").count(), 1);
    }

    #[test]
    fn pipeline_exhaustion_reason_appears_once_in_markdown() {
        let commit = CommitHash::new("abc1234").unwrap();
        let output = render(
            PipelineReviewResult {
                summary: review_summary(),
                ordered_commits: vec![commit.clone()],
                stages: vec![PipelineStageResult::ReviewContext(exhausted_stage_run(
                    crate::stage::StageKind::ReviewContext,
                    StageTarget::Commit(commit),
                    &[],
                ))],
                errors: Vec::new(),
            }
            .into(),
            RenderOptions::from_cli(OutputFormat::Markdown, None).unwrap(),
        )
        .unwrap();

        assert_eq!(output.matches("iteration limit reached").count(), 1);
    }

    #[test]
    fn pipeline_exhaustion_reason_appears_once_in_github() {
        let commit = CommitHash::new("abc1234").unwrap();
        let output = render(
            PipelineReviewResult {
                summary: review_summary(),
                ordered_commits: vec![commit.clone()],
                stages: vec![PipelineStageResult::ReviewContext(exhausted_stage_run(
                    crate::stage::StageKind::ReviewContext,
                    StageTarget::Commit(commit),
                    &[],
                ))],
                errors: Vec::new(),
            }
            .into(),
            RenderOptions::from_cli(OutputFormat::Github, Some("owner/repo".to_string())).unwrap(),
        )
        .unwrap();

        assert_eq!(output.matches("iteration limit reached").count(), 1);
    }
}
