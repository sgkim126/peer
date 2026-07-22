use std::fmt::Write;

use crate::llm::result::{CheckResult, CheckTarget, CheckUsage, Finding, Severity};

use super::escape_markdown;

pub fn render(result: &CheckResult) -> String {
    let mut output = String::new();
    writeln!(output, "## Check: {}", escape_markdown(&result.check)).unwrap();
    writeln!(output).unwrap();
    writeln!(
        output,
        "- **Target:** {}",
        escape_markdown(display_target(&result.target))
    )
    .unwrap();
    writeln!(output, "- **Status:** {}", status(result)).unwrap();
    writeln!(output).unwrap();
    writeln!(output, "{}", escape_markdown(&result.summary)).unwrap();
    writeln!(output).unwrap();
    writeln!(output, "### Findings").unwrap();
    writeln!(output).unwrap();
    if result.findings.is_empty() {
        writeln!(output, "None.").unwrap();
    } else {
        for finding in &result.findings {
            writeln!(output, "- {}", render_finding(finding)).unwrap();
        }
    }
    if result.is_exhausted {
        writeln!(output).unwrap();
        writeln!(output, "> [!WARNING]").unwrap();
        writeln!(
            output,
            "> {}",
            escape_markdown(
                result
                    .exhaustion_reason
                    .as_deref()
                    .unwrap_or("check did not complete")
            )
        )
        .unwrap();
    }
    writeln!(output).unwrap();
    writeln!(output, "### Metadata").unwrap();
    writeln!(output).unwrap();
    writeln!(output, "- **Iterations:** {}", result.iterations).unwrap();
    write_usage_markdown(&mut output, &result.usage);
    output.trim_end().to_string()
}

fn render_finding(finding: &Finding) -> String {
    format!(
        "**{}** — {} ({})",
        severity_name(finding.severity),
        escape_markdown(&finding.message),
        escape_markdown(&finding_context(finding))
    )
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

fn write_usage_markdown(output: &mut String, usage: &CheckUsage) {
    writeln!(output).unwrap();
    writeln!(output, "### Usage").unwrap();
    writeln!(output).unwrap();
    writeln!(output, "- **Input tokens:** {}", usage.input_tokens).unwrap();
    writeln!(output, "- **Output tokens:** {}", usage.output_tokens).unwrap();
    writeln!(output, "- **Cost:** ${:.6}", usage.cost_usd).unwrap();
    writeln!(output, "- **Model:** {}", escape_markdown(&usage.model)).unwrap();
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
    fn info_findings_produce_a_warning_status() {
        let mut result = result();
        result.findings = vec![result.findings.pop().unwrap()];
        result.findings[0].severity = Severity::Info;
        assert!(render(&result).contains("- **Status:** warning"));
    }

    #[test]
    fn failed_results_render_the_failure_and_usage() {
        let mut result = result();
        result.findings.clear();
        result.is_exhausted = true;
        result.exhaustion_reason = Some("transient LLM call failure: request timed out".into());
        let output = render(&result);

        assert!(output.contains("- **Status:** failed"));
        assert!(output.contains("> transient LLM call failure: request timed out"));
        assert!(output.contains("- **Input tokens:** 100"));
    }

    #[test]
    fn includes_usage() {
        let output = render(&result());

        assert!(output.contains("### Usage"));
        assert!(output.contains("- **Cost:** $0.001000"));
    }

    #[test]
    fn escapes_dynamic_content_and_flattens_newlines() {
        let mut result = result();
        result.check = "check <unsafe>".to_string();
        result.summary = "Summary\n# injected heading [link](url) ~struck~".to_string();
        result.findings[0].message = "message\n- injected finding **bold**".to_string();
        result.findings[0].location = Some(FileLocation {
            file: "src/[unsafe].rs".to_string(),
            line: None,
        });

        let output = render(&result);

        assert!(output.contains("## Check: check \\<unsafe\\>"));
        assert!(output.contains("Summary \\# injected heading \\[link\\]\\(url\\) \\~struck\\~"));
        assert!(output.contains("message \\- injected finding \\*\\*bold\\*\\*"));
        assert!(output.contains("src/\\[unsafe\\]\\.rs"));
        assert!(!output.contains("\n# injected heading"));
        assert!(!output.contains("\n- injected finding"));
    }
}
