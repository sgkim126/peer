use std::collections::BTreeMap;
use std::fmt::Write;

use crate::llm::result::{CheckResult, CheckTarget, Finding, LlmUsage, Severity};
use crate::review::{ModelUsage, ReviewSummary};

use super::escape_markdown;

pub fn render(result: &CheckResult) -> String {
    let mut output = String::new();
    writeln!(output, "## Check: {}", escape_markdown(&result.check)).unwrap();
    writeln!(output).unwrap();
    writeln!(
        output,
        "- **Target:** {}",
        escape_markdown(&display_target(&result.target))
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
    if let Some(usage) = &result.context_usage {
        write_usage_markdown(&mut output, "Context usage", usage);
    }
    write_usage_markdown(&mut output, "Check usage", &result.usage);
    output.trim_end().to_string()
}

pub fn render_review_summary(
    summary: &ReviewSummary,
    context_usage: Option<&LlmUsage>,
    usage_by_model: &BTreeMap<String, ModelUsage>,
) -> String {
    let mut output = format!(
        "## Review summary\n\n- **Peer version:** {}\n- **Provider:** {}\n- **Model:** {}",
        summary.peer_version, summary.provider, summary.model,
    );
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

fn write_usage_markdown(output: &mut String, heading: &str, usage: &LlmUsage) {
    writeln!(output).unwrap();
    writeln!(output, "### {heading}").unwrap();
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
            is_exhausted: false,
            exhaustion_reason: None,
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

        assert!(output.contains("### Check usage"));
        assert!(output.contains("- **Cost:** $0.001000"));
    }

    #[test]
    fn includes_context_usage_separately() {
        let mut result = result();
        result.context_usage = Some(LlmUsage {
            input_tokens: 40,
            output_tokens: 10,
            cost_usd: 0.0004,
            model: "contextmodel".to_string(),
        });

        let output = render(&result);

        assert!(output.contains("### Context usage"));
        assert!(output.contains("### Check usage"));
        assert!(output.contains("- **Model:** contextmodel"));
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
