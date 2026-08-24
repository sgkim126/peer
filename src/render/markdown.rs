use std::collections::BTreeMap;
use std::fmt::Write;

use crate::git::CommitHash;
use crate::llm::LlmUsage;
use crate::review::{ModelUsage, ReviewSummary};
use crate::stage::{
    FileLocation, KnowledgeQuestion, Severity, StageFailure, StageTarget, StructuralRecommendation,
};
#[cfg(test)]
use crate::stage::{Finding, StageResult};

use super::{
    RenderDocument, RenderFinding, RenderInput, RenderStage, RenderStageErrorRef, ReviewCounts,
    clarification_message, escape_markdown, join_review_sections, review_counts, usage_by_model,
};

pub fn render(input: &RenderInput) -> String {
    match input {
        RenderInput::Document(document) => render_document(document),
        RenderInput::KnowledgeQuestion(question) => render_question(question),
        RenderInput::StructuralRecommendation(recommendation) => {
            render_recommendation(recommendation)
        }
        RenderInput::Finding(finding) => render_finding(finding),
    }
}

fn render_document(document: &RenderDocument) -> String {
    let stages = document
        .stages
        .iter()
        .map(render_stage)
        .collect::<Vec<_>>()
        .join("\n\n");
    let summary = document.summary.as_ref().map(|summary| {
        render_review_summary(
            summary,
            document.context_usage.as_ref(),
            &usage_by_model(document),
            &review_counts(document),
        )
    });
    let context = document
        .summary
        .is_none()
        .then_some(document.context_usage.as_ref())
        .flatten()
        .map(render_context_usage);
    let questions = render_questions(&document.questions);
    let recommendations = render_recommendations(&document.recommendations);
    let findings = render_findings(&document.findings);
    join_review_sections(
        summary
            .into_iter()
            .chain([questions, recommendations, findings])
            .chain(context)
            .chain([stages]),
    )
}

fn render_stage(result: &RenderStage) -> String {
    let mut output = String::new();
    writeln!(output, "## Stage: {}", escape_markdown(&result.stage)).unwrap();
    writeln!(output).unwrap();
    writeln!(
        output,
        "- **Target:** {}",
        escape_markdown(&display_target(&result.target))
    )
    .unwrap();
    writeln!(output, "- **Status:** {}", result.status()).unwrap();
    writeln!(output).unwrap();
    if let Some(summary) = result.summary() {
        writeln!(output, "{}", escape_markdown(summary)).unwrap();
        writeln!(output).unwrap();
    }
    if let Some(error) = result.error() {
        writeln!(output).unwrap();
        writeln!(output, "> [!WARNING]").unwrap();
        writeln!(output, "> {}", escape_markdown(&display_error(error))).unwrap();
    }
    let iterations = result.iterations();
    let usage = result.usage();
    if iterations.is_some() || usage.is_some() {
        writeln!(output).unwrap();
        writeln!(output, "### Metadata").unwrap();
        writeln!(output).unwrap();
        if let Some(iterations) = iterations {
            writeln!(output, "- **Iterations:** {iterations}").unwrap();
        }
        if let Some(usage) = usage {
            write_usage_markdown(&mut output, "Stage usage", usage);
        }
    }
    output.trim_end().to_string()
}

fn render_context_usage(usage: &LlmUsage) -> String {
    let mut output = String::new();
    write_usage_markdown(&mut output, "Context usage", usage);
    output.trim().to_string()
}

fn render_review_summary(
    summary: &ReviewSummary,
    context_usage: Option<&LlmUsage>,
    usage_by_model: &BTreeMap<String, ModelUsage>,
    counts: &ReviewCounts,
) -> String {
    let mut output = format!(
        "## Review summary\n\n- **Peer version:** {}\n- **Provider:** {}\n- **Model:** {}",
        summary.peer_version, summary.provider, summary.model,
    );
    write!(
        output,
        "\n- **Info findings:** {}\n- **Low findings:** {}\n- **Medium findings:** {}\n- **High findings:** {}\n- **Critical findings:** {}\n- **Exhausted stages:** {}\n- **Failed stages:** {}",
        counts.info,
        counts.low,
        counts.medium,
        counts.high,
        counts.critical,
        counts.exhausted,
        counts.failed,
    )
    .unwrap();
    if let Some(usage) = context_usage {
        write!(
            output,
            "\n- **Context usage:** {} input tokens, {} output tokens, ${:.6} ({})",
            usage.input_tokens,
            usage.output_tokens,
            usage.cost_usd,
            escape_markdown(&usage.model),
        )
        .unwrap();
    }
    output.push_str("\n\n### Total token usage\n\n");
    if usage_by_model.is_empty() {
        output.push_str("None.");
    } else {
        for (model, usage) in usage_by_model {
            writeln!(
                output,
                "- **{}:** {} input tokens, {} output tokens, ${:.6}",
                escape_markdown(model),
                usage.input_tokens,
                usage.output_tokens,
                usage.cost_usd,
            )
            .unwrap();
        }
    }
    output.trim_end().to_string()
}

fn display_error(error: RenderStageErrorRef<'_>) -> String {
    match error {
        RenderStageErrorRef::Exhausted(reason) | RenderStageErrorRef::Execution(reason) => {
            reason.to_string()
        }
        RenderStageErrorRef::Stage(StageFailure::ClarificationRequired { questions }) => {
            clarification_message(questions)
        }
        RenderStageErrorRef::Stage(error) => error.to_string(),
    }
}

fn display_target(target: &StageTarget) -> String {
    match target {
        StageTarget::Commit(commit) => commit.to_string(),
        StageTarget::Range { from, to } => format!("{from}..{to}"),
    }
}

fn severity_name(severity: Severity) -> &'static str {
    match severity {
        Severity::Info => "info",
        Severity::Low => "low",
        Severity::Medium => "medium",
        Severity::High => "high",
        Severity::Critical => "critical",
    }
}

fn render_questions(questions: &[KnowledgeQuestion]) -> String {
    if questions.is_empty() {
        return String::new();
    }
    let mut output = "## Review questions\n".to_string();
    for question in questions {
        writeln!(output, "{}", render_question(question)).unwrap();
    }
    output.trim_end().to_string()
}

fn render_question(question: &KnowledgeQuestion) -> String {
    format!(
        "- **question/{}** — {} Evidence: {} Why it matters: {} ({})",
        question.category.as_str(),
        escape_markdown(&question.question),
        escape_markdown(&question.evidence),
        escape_markdown(&question.why_it_matters),
        escape_markdown(&related_context(
            &question.related_commits,
            question.location.as_ref(),
        )),
    )
}

fn render_recommendations(recommendations: &[StructuralRecommendation]) -> String {
    if recommendations.is_empty() {
        return String::new();
    }
    let mut output = "## Structural recommendations\n".to_string();
    for recommendation in recommendations {
        writeln!(output, "{}", render_recommendation(recommendation)).unwrap();
    }
    output.trim_end().to_string()
}

fn render_recommendation(recommendation: &StructuralRecommendation) -> String {
    format!(
        "- **recommendation/{}** — {} Rationale: {} ({})",
        recommendation.kind.as_str(),
        escape_markdown(&recommendation.message),
        escape_markdown(&recommendation.rationale),
        escape_markdown(&related_context(&recommendation.related_commits, None)),
    )
}

fn render_findings(findings: &[RenderFinding]) -> String {
    if findings.is_empty() {
        return String::new();
    }
    let mut output = "## Review findings\n".to_string();
    for finding in findings {
        writeln!(output, "{}", render_finding(finding)).unwrap();
    }
    output.trim_end().to_string()
}

fn render_finding(finding: &RenderFinding) -> String {
    let mut output = format!(
        "- **finding/{}** — {}",
        severity_name(finding.severity),
        escape_markdown(&finding.message),
    );
    if let Some(security) = &finding.security {
        write!(
            output,
            " Attacker control: {} Sensitive operation: {} Impact: {}",
            escape_markdown(&security.attacker_control),
            escape_markdown(&security.sensitive_operation),
            escape_markdown(&security.impact),
        )
        .unwrap();
    }
    write!(
        output,
        " ({})",
        escape_markdown(&finding_context(&finding.commit, finding.location.as_ref(),))
    )
    .unwrap();
    output
}

fn related_context(
    commits: &[CommitHash],
    location: Option<&crate::stage::KnowledgeLocation>,
) -> String {
    commits
        .iter()
        .map(|commit| {
            let file = location
                .filter(|location| location.commit.matches(commit))
                .map(|location| &location.file);
            finding_context(commit, file)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn finding_context(commit: &CommitHash, location: Option<&FileLocation>) -> String {
    match location {
        Some(location) => match location.line {
            Some(line) => format!("{commit} · {}:{line}", location.file),
            None => format!("{commit} · {}", location.file),
        },
        None => commit.to_string(),
    }
}

fn write_usage_markdown(output: &mut String, heading: &str, usage: &LlmUsage) {
    writeln!(output).unwrap();
    writeln!(output, "### {heading}").unwrap();
    writeln!(output).unwrap();
    writeln!(output, "- **Input tokens:** {}", usage.input_tokens).unwrap();
    writeln!(output, "- **Output tokens:** {}", usage.output_tokens).unwrap();
    if usage.cache_read_tokens != 0 || usage.cache_write_tokens != 0 {
        writeln!(
            output,
            "- **Cache-read tokens:** {}",
            usage.cache_read_tokens
        )
        .unwrap();
        writeln!(
            output,
            "- **Cache-write tokens:** {}",
            usage.cache_write_tokens
        )
        .unwrap();
    }
    writeln!(output, "- **Cost:** ${:.6}", usage.cost_usd).unwrap();
    writeln!(output, "- **Model:** {}", escape_markdown(&usage.model)).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::git::CommitHash;
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

    #[test]
    fn info_findings_produce_a_warning_status() {
        let mut result = result();
        result.findings = vec![result.findings.pop().unwrap()];
        result.findings[0].severity = Severity::Info;
        let rendered = crate::render::RenderStageParts::from(result);
        assert!(render_stage(&rendered.stage).contains("- **Status:** issues"));
    }

    #[test]
    fn failed_results_render_the_failure_and_usage() {
        let mut result = result();
        result.findings.clear();
        result.failure = Some(StageFailure::ClarificationRequired {
            questions: vec![crate::stage::ClarificationQuestion {
                question: "Which deployment policy applies?".into(),
                reason: "The policy affects the finding.".into(),
            }],
        });
        let rendered = crate::render::RenderStageParts::from(result);
        let output = render_stage(&rendered.stage);

        assert!(output.contains("- **Status:** failed"));
        assert!(output.contains("> Which deployment policy applies?"));
        assert!(output.contains("- **Input tokens:** 100"));
    }

    #[test]
    fn includes_usage() {
        let rendered = crate::render::RenderStageParts::from(result());
        let output = render_stage(&rendered.stage);

        assert!(output.contains("### Stage usage"));
        assert!(output.contains("- **Cost:** $0.001000"));
        assert!(!output.contains("Cache-read tokens"));
        assert!(!output.contains("Cache-write tokens"));
    }

    #[test]
    fn includes_cache_usage_when_available() {
        let mut result = result();
        result.usage.cache_read_tokens = 80;
        result.usage.cache_write_tokens = 10;

        let rendered = crate::render::RenderStageParts::from(result);
        let output = render_stage(&rendered.stage);

        assert!(output.contains("- **Cache-read tokens:** 80"));
        assert!(output.contains("- **Cache-write tokens:** 10"));
    }

    #[test]
    fn execution_failure_omits_empty_metadata() {
        let stage = RenderStage {
            stage: "quality".into(),
            target: StageTarget::Commit(CommitHash::new("abc1234").unwrap()),
            outcome: crate::render::RenderStageOutcome::Failed {
                failure: crate::render::RenderStageFailure::Execution {
                    reason: "provider unavailable".into(),
                    usage: None,
                },
            },
        };

        assert!(!render_stage(&stage).contains("### Metadata"));
    }

    #[test]
    fn execution_failure_renders_metadata_with_usage() {
        let stage = RenderStage {
            stage: "quality".into(),
            target: StageTarget::Commit(CommitHash::new("abc1234").unwrap()),
            outcome: crate::render::RenderStageOutcome::Failed {
                failure: crate::render::RenderStageFailure::Execution {
                    reason: "invalid typed stage report".into(),
                    usage: Some(result().usage),
                },
            },
        };

        assert!(render_stage(&stage).contains("### Metadata"));
    }

    #[test]
    fn includes_context_usage_separately() {
        let mut result = result();
        result.context_usage = Some(LlmUsage {
            input_tokens: 40,
            output_tokens: 10,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_usd: 0.0004,
            model: "contextmodel".to_string(),
            models: Vec::new(),
        });

        let output = render_context_usage(result.context_usage.as_ref().unwrap());

        assert!(output.contains("### Context usage"));
        assert!(output.contains("- **Model:** contextmodel"));
    }

    #[test]
    fn escapes_dynamic_content_and_flattens_newlines() {
        let mut result = result();
        result.stage = "stage <unsafe>".to_string();
        result.summary = "Summary\n# injected heading [link](url) ~struck~".to_string();
        result.findings[0].message = "message\n- injected finding **bold**".to_string();
        result.findings[0].location = Some(FileLocation {
            file: "src/[unsafe].rs".to_string(),
            line: None,
        });

        let rendered = crate::render::RenderStageParts::from(result);
        let stage_output = render_stage(&rendered.stage);
        let findings_output = render_findings(&rendered.findings);

        assert!(stage_output.contains("## Stage: stage \\<unsafe\\>"));
        assert!(
            stage_output.contains("Summary \\# injected heading \\[link\\]\\(url\\) \\~struck\\~")
        );
        assert!(findings_output.contains("message \\- injected finding \\*\\*bold\\*\\*"));
        assert!(findings_output.contains("src/\\[unsafe\\]\\.rs"));
        assert!(!stage_output.contains("\n# injected heading"));
        assert!(!findings_output.contains("\n- injected finding"));
    }

    #[test]
    fn renders_findings_as_a_tight_list() {
        let findings = vec![
            Finding {
                commit: CommitHash::new("abc1234").unwrap(),
                severity: Severity::High,
                message: "First.".to_string(),
                location: None,
            },
            Finding {
                commit: CommitHash::new("def5678").unwrap(),
                severity: Severity::Low,
                message: "Second.".to_string(),
                location: None,
            },
        ];

        let findings = findings
            .into_iter()
            .map(crate::render::render_finding)
            .collect::<Vec<_>>();
        let output = render_findings(&findings);

        assert!(output.starts_with("## Review findings\n- "));
        assert!(!output.contains("\n\n- "));
    }
}
