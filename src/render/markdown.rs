use std::collections::BTreeMap;
use std::fmt::Write;

#[cfg(test)]
use crate::llm::CheckResult;
use crate::llm::{CheckError, CheckTarget, Finding, LlmUsage, Severity};
use crate::review::{ModelUsage, ReviewSummary};

use super::{RenderCheck, RenderCheckErrorRef, ReviewCounts, escape_markdown};

pub fn render(result: &RenderCheck) -> String {
    let mut output = String::new();
    writeln!(output, "## Check: {}", escape_markdown(&result.check)).unwrap();
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
    writeln!(output, "### Findings").unwrap();
    writeln!(output).unwrap();
    if result.findings().is_empty() {
        writeln!(output, "None.").unwrap();
    } else {
        for finding in result.findings() {
            writeln!(output, "- {}", render_finding(finding)).unwrap();
        }
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
            write_usage_markdown(&mut output, "Check usage", usage);
        }
    }
    output.trim_end().to_string()
}

pub fn render_context_usage(usage: &LlmUsage) -> String {
    let mut output = String::new();
    write_usage_markdown(&mut output, "Context usage", usage);
    output.trim().to_string()
}

pub fn render_review_summary(
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
        "\n- **Info findings:** {}\n- **Low findings:** {}\n- **Medium findings:** {}\n- **High findings:** {}\n- **Critical findings:** {}\n- **Exhausted checks:** {}\n- **Failed checks:** {}",
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

fn render_finding(finding: &Finding) -> String {
    format!(
        "**{}** — {} ({})",
        severity_name(finding.severity),
        escape_markdown(&finding.message),
        escape_markdown(&finding_context(finding))
    )
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
    use crate::llm::FileLocation;

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
    fn info_findings_produce_a_warning_status() {
        let mut result = result();
        result.findings = vec![result.findings.pop().unwrap()];
        result.findings[0].severity = Severity::Info;
        assert!(render(&result.into()).contains("- **Status:** issues"));
    }

    #[test]
    fn failed_results_render_the_failure_and_usage() {
        let mut result = result();
        result.findings.clear();
        result.error = Some(CheckError::Agent {
            reason: "transient LLM call failure: request timed out".into(),
        });
        let output = render(&result.into());

        assert!(output.contains("- **Status:** failed"));
        assert!(output.contains("> transient LLM call failure: request timed out"));
        assert!(output.contains("- **Input tokens:** 100"));
    }

    #[test]
    fn includes_usage() {
        let output = render(&result().into());

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

        let output = render_context_usage(result.context_usage.as_ref().unwrap());

        assert!(output.contains("### Context usage"));
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

        let output = render(&result.into());

        assert!(output.contains("## Check: check \\<unsafe\\>"));
        assert!(output.contains("Summary \\# injected heading \\[link\\]\\(url\\) \\~struck\\~"));
        assert!(output.contains("message \\- injected finding \\*\\*bold\\*\\*"));
        assert!(output.contains("src/\\[unsafe\\]\\.rs"));
        assert!(!output.contains("\n# injected heading"));
        assert!(!output.contains("\n- injected finding"));
    }
}
