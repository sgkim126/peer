use std::collections::BTreeMap;
use std::fmt::{self, Write};

use owo_colors::Style;

use crate::llm::result::{CheckError, CheckTarget, Finding, LlmUsage, Severity};
use crate::review::{ModelUsage, ReviewSummary};

use super::{RenderCheck, RenderCheckErrorRef, escape_terminal};

#[cfg(test)]
use crate::llm::result::CheckResult;

pub fn render(result: &RenderCheck, use_color: bool) -> String {
    let mut output = String::new();
    writeln!(
        output,
        "{} {}",
        label("Check:", use_color),
        bold(&escape_terminal(&result.check), use_color)
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
    writeln!(output, "{}", label("Findings:", use_color)).unwrap();
    if result.findings().is_empty() {
        writeln!(output, "- none").unwrap();
    } else {
        for finding in result.findings() {
            writeln!(output, "- {}", render_finding(finding, use_color)).unwrap();
        }
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
        write_usage(&mut output, "Check usage", usage);
    }
    output
}

pub fn render_context_usage(usage: &LlmUsage) -> String {
    let mut output = String::new();
    write_usage(&mut output, "Context usage", usage);
    output.trim().to_string()
}

pub fn render_review_summary(
    summary: &ReviewSummary,
    context_usage: Option<&LlmUsage>,
    usage_by_model: &BTreeMap<String, ModelUsage>,
    use_color: bool,
) -> String {
    let mut output = String::new();
    writeln!(output, "{}", label("Review summary:", use_color)).unwrap();
    writeln!(output, "- Peer version: {}", summary.peer_version).unwrap();
    writeln!(output, "- Provider: {}", summary.provider).unwrap();
    writeln!(output, "- Model: {}", summary.model).unwrap();
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

fn render_finding(finding: &Finding, use_color: bool) -> String {
    format!(
        "[{}] {} ({})",
        styled(
            severity_name(finding.severity),
            severity_style(finding.severity),
            use_color
        ),
        escape_terminal(&finding.message),
        styled(
            escape_terminal(&finding_context(finding)),
            Style::new().dimmed(),
            use_color
        )
    )
}

fn write_usage(output: &mut String, label: &str, usage: &LlmUsage) {
    writeln!(output).unwrap();
    write!(
        output,
        "{label}: {} input, {} output, ${:.6} ({})",
        usage.input_tokens,
        usage.output_tokens,
        usage.cost_usd,
        escape_terminal(&usage.model)
    )
    .unwrap();
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

fn display_error(error: RenderCheckErrorRef<'_>) -> String {
    match error {
        RenderCheckErrorRef::Exhausted(reason) | RenderCheckErrorRef::Execution(reason) => {
            reason.to_string()
        }
        RenderCheckErrorRef::Check(CheckError::ClarificationRequired { questions }) => {
            questions.join("; ")
        }
        RenderCheckErrorRef::Check(error) => error.to_string(),
    }
}

fn display_target(target: &CheckTarget) -> String {
    match target {
        CheckTarget::Commit(commit) => commit.to_string(),
        CheckTarget::Range { from, to } => format!("{from}..{to}"),
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

fn finding_context(finding: &Finding) -> String {
    match &finding.location {
        Some(location) => match location.line {
            Some(line) => format!("{} · {}:{line}", finding.commit, location.file),
            None => format!("{} · {}", finding.commit, location.file),
        },
        None => finding.commit.to_string(),
    }
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn has_no_ansi_codes_without_tty() {
        let output = render(&result().into(), false);

        assert!(output.contains("Check usage: 100 input, 20 output, $0.001000 (test-model)"));
        assert!(!output.contains("\u{1b}["));
    }

    #[test]
    fn includes_context_usage_separately() {
        let mut result = result();
        result.context_usage = Some(LlmUsage {
            input_tokens: 40,
            output_tokens: 10,
            cost_usd: 0.0004,
            model: "context-model".to_string(),
        });

        let output = render_context_usage(result.context_usage.as_ref().unwrap());

        assert!(output.contains("Context usage: 40 input, 10 output, $0.000400 (context-model)"));
    }

    #[test]
    fn escapes_control_characters_and_flattens_newlines() {
        let mut result = result();
        result.check = "security\u{1b}[2J".to_string();
        result.summary = "Summary\nforged output\u{7}".to_string();
        result.findings[0].message = "message\u{1b}[31m\nnext line".to_string();

        let output = render(&result.into(), true);

        assert!(output.contains(r"security\u{1b}[2J"));
        assert!(output.contains(r"Summary forged output\u{7}"));
        assert!(output.contains(r"message\u{1b}[31m next line"));
        assert!(!output.contains("security\u{1b}[2J"));
        assert!(!output.contains("message\u{1b}[31m"));
        assert!(!output.contains("\nforged output"));
        assert!(!output.contains("\nnext line"));
    }

    #[test]
    fn preserves_unicode_text() {
        let mut result = result();
        result.summary = "변경 사항을 확인했습니다.".to_string();

        let output = render(&result.into(), false);

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
