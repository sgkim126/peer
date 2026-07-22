use std::fmt::{self, Write};

use owo_colors::Style;

use crate::llm::result::{CheckResult, CheckTarget, CheckUsage, Finding, Severity};

pub fn render(result: &CheckResult, use_color: bool) -> String {
    let mut output = String::new();
    writeln!(
        output,
        "{} {}",
        label("Check:", use_color),
        bold(&result.check, use_color)
    )
    .unwrap();
    writeln!(
        output,
        "{} {}",
        label("Target:", use_color),
        display_target(&result.target)
    )
    .unwrap();
    writeln!(
        output,
        "{} {}",
        label("Status:", use_color),
        styled(status(result), status_style(status(result)), use_color)
    )
    .unwrap();
    writeln!(output).unwrap();
    writeln!(output, "{}", result.summary).unwrap();
    writeln!(output).unwrap();
    writeln!(output, "{}", label("Findings:", use_color)).unwrap();
    if result.findings.is_empty() {
        writeln!(output, "- none").unwrap();
    } else {
        for finding in &result.findings {
            writeln!(output, "- {}", render_finding(finding, use_color)).unwrap();
        }
    }
    if result.is_exhausted {
        writeln!(output).unwrap();
        writeln!(
            output,
            "{} {}",
            styled("Warning:", Style::new().yellow().bold(), use_color),
            result
                .exhaustion_reason
                .as_deref()
                .unwrap_or("unknown reason")
        )
        .unwrap();
    }
    writeln!(output).unwrap();
    writeln!(
        output,
        "{} {}",
        label("Iterations:", use_color),
        result.iterations
    )
    .unwrap();
    write_usage(&mut output, &result.usage);
    output
}

fn render_finding(finding: &Finding, use_color: bool) -> String {
    format!(
        "[{}] {} ({})",
        styled(
            severity_name(finding.severity),
            severity_style(finding.severity),
            use_color
        ),
        finding.message,
        styled(finding_context(finding), Style::new().dimmed(), use_color)
    )
}

fn write_usage(output: &mut String, usage: &CheckUsage) {
    writeln!(output).unwrap();
    write!(
        output,
        "Usage: {} input, {} output, ${:.6} ({})",
        usage.input_tokens, usage.output_tokens, usage.cost_usd, usage.model
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
        "ok" => Style::new().green().bold(),
        "warning" => Style::new().yellow().bold(),
        "issue" | "failed" => Style::new().red().bold(),
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

fn status(result: &CheckResult) -> &'static str {
    if result.is_exhausted {
        return "failed";
    }
    match result.findings.iter().map(|finding| finding.severity).max() {
        Some(Severity::Critical | Severity::High) => "issue",
        Some(Severity::Info | Severity::Low | Severity::Medium) => "warning",
        None => "ok",
    }
}

fn display_target(target: &CheckTarget) -> &str {
    match target {
        CheckTarget::Commit(commit) => commit.as_ref(),
        CheckTarget::Range(range) => range,
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
    use crate::llm::result::{CheckTarget, CheckUsage, FileLocation, Finding, Severity};

    fn result() -> CheckResult {
        CheckResult {
            check: "security".to_string(),
            target: CheckTarget::Range("HEAD~2..HEAD".to_string()),
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
            is_exhausted: false,
            exhaustion_reason: None,
            usage: CheckUsage {
                input_tokens: 100,
                output_tokens: 20,
                cost_usd: 0.001,
                model: "test-model".to_string(),
            },
        }
    }

    #[test]
    fn has_no_ansi_codes_without_tty() {
        let output = render(&result(), false);

        assert!(output.contains("Usage: 100 input, 20 output, $0.001000 (test-model)"));
        assert!(!output.contains("\u{1b}["));
    }
}
