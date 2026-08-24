use std::collections::BTreeMap;
use std::fmt::{self, Write};
use std::io::IsTerminal;

use owo_colors::Style;

use crate::git::CommitHash;
use crate::llm::LlmUsage;
use crate::review::{ModelUsage, ReviewSummary};
use crate::stage::{
    FileLocation, KnowledgeQuestion, Severity, StageFailure, StageTarget, StructuralRecommendation,
};
#[cfg(test)]
use crate::stage::{Finding, StageResult};

use super::{
    RenderDocument, RenderFinding, RenderStage, RenderStageErrorRef, ReviewCounts,
    clarification_message, escape_terminal, join_review_sections, review_counts, usage_by_model,
};

pub fn render(document: &RenderDocument) -> String {
    let use_color = std::io::stdout().is_terminal();
    let stages = document
        .stages
        .iter()
        .map(|stage| render_stage(stage, use_color))
        .collect::<Vec<_>>()
        .join("\n\n");
    let summary = document.summary.as_ref().map(|summary| {
        render_review_summary(
            summary,
            document.context_usage.as_ref(),
            &usage_by_model(document),
            &review_counts(document),
            use_color,
        )
    });
    let context = document
        .summary
        .is_none()
        .then_some(document.context_usage.as_ref())
        .flatten()
        .map(render_context_usage);
    let questions = render_questions(&document.questions, use_color);
    let recommendations = render_recommendations(&document.recommendations, use_color);
    let findings = render_findings(&document.findings, use_color);
    join_review_sections(
        summary
            .into_iter()
            .chain([questions, recommendations, findings])
            .chain(context)
            .chain([stages]),
    )
}

fn render_stage(result: &RenderStage, use_color: bool) -> String {
    let mut output = String::new();
    writeln!(
        output,
        "{} {}",
        label("Stage:", use_color),
        bold(&escape_terminal(&result.stage), use_color)
    )
    .unwrap();
    writeln!(
        output,
        "{} {}",
        label("Target:", use_color),
        escape_terminal(&display_target(&result.target))
    )
    .unwrap();
    writeln!(
        output,
        "{} {}",
        label("Status:", use_color),
        styled(result.status(), status_style(result.status()), use_color)
    )
    .unwrap();
    writeln!(output).unwrap();
    if let Some(summary) = result.summary() {
        writeln!(output, "{}", escape_terminal(summary)).unwrap();
        writeln!(output).unwrap();
    }
    if let Some(error) = result.error() {
        writeln!(output).unwrap();
        writeln!(
            output,
            "{} {}",
            styled("Warning:", Style::new().yellow().bold(), use_color),
            escape_terminal(&display_error(error))
        )
        .unwrap();
    }
    if let Some(iterations) = result.iterations() {
        writeln!(output).unwrap();
        writeln!(output, "{} {}", label("Iterations:", use_color), iterations).unwrap();
    }
    if let Some(usage) = result.usage() {
        write_usage(&mut output, "Stage usage", usage);
    }
    output
}

fn render_context_usage(usage: &LlmUsage) -> String {
    let mut output = String::new();
    write_usage(&mut output, "Context usage", usage);
    output.trim().to_string()
}

fn render_review_summary(
    summary: &ReviewSummary,
    context_usage: Option<&LlmUsage>,
    usage_by_model: &BTreeMap<String, ModelUsage>,
    counts: &ReviewCounts,
    use_color: bool,
) -> String {
    let mut output = String::new();
    writeln!(output, "{}", label("Review summary:", use_color)).unwrap();
    writeln!(output, "- Peer version: {}", summary.peer_version).unwrap();
    writeln!(output, "- Provider: {}", summary.provider).unwrap();
    writeln!(output, "- Model: {}", summary.model).unwrap();
    writeln!(output, "- Info findings: {}", counts.info).unwrap();
    writeln!(output, "- Low findings: {}", counts.low).unwrap();
    writeln!(output, "- Medium findings: {}", counts.medium).unwrap();
    writeln!(output, "- High findings: {}", counts.high).unwrap();
    writeln!(output, "- Critical findings: {}", counts.critical).unwrap();
    writeln!(output, "- Exhausted stages: {}", counts.exhausted).unwrap();
    writeln!(output, "- Failed stages: {}", counts.failed).unwrap();
    if let Some(usage) = context_usage {
        writeln!(
            output,
            "- Context usage: {} input, {} output, ${:.6} ({})",
            usage.input_tokens,
            usage.output_tokens,
            usage.cost_usd,
            escape_terminal(&usage.model),
        )
        .unwrap();
    }
    writeln!(output, "- Total token usage:").unwrap();
    if usage_by_model.is_empty() {
        write!(output, "  - none").unwrap();
    } else {
        for (model, usage) in usage_by_model {
            writeln!(
                output,
                "  - {}: {} input, {} output, ${:.6}",
                escape_terminal(model),
                usage.input_tokens,
                usage.output_tokens,
                usage.cost_usd,
            )
            .unwrap();
        }
    }
    output.trim_end().to_string()
}

fn write_usage(output: &mut String, label: &str, usage: &LlmUsage) {
    writeln!(output).unwrap();
    if usage.cache_read_tokens == 0 && usage.cache_write_tokens == 0 {
        write!(
            output,
            "{label}: {} input, {} output, ${:.6} ({})",
            usage.input_tokens,
            usage.output_tokens,
            usage.cost_usd,
            escape_terminal(&usage.model)
        )
        .unwrap();
    } else {
        write!(
            output,
            "{label}: {} input, {} output, {} cache read, {} cache write, ${:.6} ({})",
            usage.input_tokens,
            usage.output_tokens,
            usage.cache_read_tokens,
            usage.cache_write_tokens,
            usage.cost_usd,
            escape_terminal(&usage.model)
        )
        .unwrap();
    }
}

fn label(value: &str, use_color: bool) -> String {
    styled(value, Style::new().bright_blue().bold(), use_color)
}

fn bold(value: &str, use_color: bool) -> String {
    styled(value, Style::new().bold(), use_color)
}

fn status_style(status: &str) -> Style {
    match status {
        "clean" => Style::new().green().bold(),
        "exhausted" => Style::new().yellow().bold(),
        "issues" | "failed" => Style::new().red().bold(),
        _ => Style::new().bold(),
    }
}

fn severity_style(severity: Severity) -> Style {
    match severity {
        Severity::Info | Severity::Low => Style::new().yellow(),
        Severity::Medium => Style::new().yellow().bold(),
        Severity::High => Style::new().red(),
        Severity::Critical => Style::new().bright_red().bold(),
    }
}

fn styled(value: impl fmt::Display, style: Style, use_color: bool) -> String {
    if use_color {
        style.style(value).to_string()
    } else {
        value.to_string()
    }
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

fn render_questions(questions: &[KnowledgeQuestion], use_color: bool) -> String {
    if questions.is_empty() {
        return String::new();
    }
    let mut output = String::new();
    writeln!(output, "{}", label("Review questions:", use_color)).unwrap();
    for question in questions {
        writeln!(
            output,
            "- [{}] {} Evidence: {} Why it matters: {} ({})",
            bold(
                &format!("question/{}", question.category.as_str()),
                use_color,
            ),
            escape_terminal(&question.question),
            escape_terminal(&question.evidence),
            escape_terminal(&question.why_it_matters),
            styled(
                escape_terminal(&related_context(
                    &question.related_commits,
                    question.location.as_ref(),
                )),
                Style::new().dimmed(),
                use_color,
            ),
        )
        .unwrap();
    }
    output.trim_end().to_string()
}

fn render_recommendations(recommendations: &[StructuralRecommendation], use_color: bool) -> String {
    if recommendations.is_empty() {
        return String::new();
    }
    let mut output = String::new();
    writeln!(
        output,
        "{}",
        label("Structural recommendations:", use_color)
    )
    .unwrap();
    for recommendation in recommendations {
        writeln!(
            output,
            "- [{}] {} Rationale: {} ({})",
            bold(
                &format!("recommendation/{}", recommendation.kind.as_str()),
                use_color,
            ),
            escape_terminal(&recommendation.message),
            escape_terminal(&recommendation.rationale),
            styled(
                escape_terminal(&related_context(&recommendation.related_commits, None)),
                Style::new().dimmed(),
                use_color,
            ),
        )
        .unwrap();
    }
    output.trim_end().to_string()
}

fn render_findings(findings: &[RenderFinding], use_color: bool) -> String {
    if findings.is_empty() {
        return String::new();
    }
    let mut output = String::new();
    writeln!(output, "{}", label("Review findings:", use_color)).unwrap();
    for finding in findings {
        write!(
            output,
            "- [finding/{}] {}",
            styled(
                severity_name(finding.severity),
                severity_style(finding.severity),
                use_color
            ),
            escape_terminal(&finding.message),
        )
        .unwrap();
        if let Some(security) = &finding.security {
            write!(
                output,
                " Attacker control: {} Sensitive operation: {} Impact: {}",
                escape_terminal(&security.attacker_control),
                escape_terminal(&security.sensitive_operation),
                escape_terminal(&security.impact),
            )
            .unwrap();
        }
        writeln!(
            output,
            " ({})",
            styled(
                escape_terminal(&finding_context(&finding.commit, finding.location.as_ref(),)),
                Style::new().dimmed(),
                use_color,
            )
        )
        .unwrap();
    }
    output.trim_end().to_string()
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
    fn has_no_ansi_codes_without_tty() {
        let rendered = crate::render::RenderStageParts::from(result());
        let output = render_stage(&rendered.stage, false);

        assert!(output.contains("Stage usage: 100 input, 20 output, $0.001000 (test-model)"));
        assert!(!output.contains("\u{1b}["));
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
            model: "context-model".to_string(),
            models: Vec::new(),
        });

        let output = render_context_usage(result.context_usage.as_ref().unwrap());

        assert!(output.contains("Context usage: 40 input, 10 output, $0.000400 (context-model)"));
    }

    #[test]
    fn colors_finding_severity() {
        let findings = vec![Finding {
            commit: CommitHash::new("abc1234").unwrap(),
            severity: Severity::High,
            message: "High-risk finding.".to_string(),
            location: None,
        }];

        let findings = findings
            .into_iter()
            .map(crate::render::render_finding)
            .collect::<Vec<_>>();
        let output = render_findings(&findings, true);
        let colored_severity = styled("high", severity_style(Severity::High), true);

        assert!(output.contains(&format!("[finding/{colored_severity}]")));
    }

    #[test]
    fn escapes_control_characters_and_flattens_newlines() {
        let mut result = result();
        result.stage = "security\u{1b}[2J".to_string();
        result.summary = "Summary\nforged output\u{7}".to_string();
        result.findings[0].message = "message\u{1b}[31m\nnext line".to_string();

        let rendered = crate::render::RenderStageParts::from(result);
        let stage_output = render_stage(&rendered.stage, true);
        let findings_output = render_findings(&rendered.findings, true);

        assert!(stage_output.contains(r"security\u{1b}[2J"));
        assert!(stage_output.contains(r"Summary forged output\u{7}"));
        assert!(findings_output.contains(r"message\u{1b}[31m next line"));
        assert!(!stage_output.contains("security\u{1b}[2J"));
        assert!(!findings_output.contains("message\u{1b}[31m"));
        assert!(!stage_output.contains("\nforged output"));
        assert!(!findings_output.contains("\nnext line"));
    }

    #[test]
    fn preserves_unicode_text() {
        let mut result = result();
        result.summary = "변경 사항을 확인했습니다.".to_string();

        let rendered = crate::render::RenderStageParts::from(result);
        let output = render_stage(&rendered.stage, false);

        assert!(output.contains("변경 사항을 확인했습니다."));
    }

    #[test]
    fn colors_current_status_names() {
        for (status, style) in [
            ("clean", Style::new().green().bold()),
            ("issues", Style::new().red().bold()),
            ("exhausted", Style::new().yellow().bold()),
            ("failed", Style::new().red().bold()),
        ] {
            assert_eq!(
                status_style(status).style(status).to_string(),
                style.style(status).to_string()
            );
        }
    }
}
