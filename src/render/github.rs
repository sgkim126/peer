use std::collections::BTreeMap;
use std::fmt::Write;

use crate::llm::result::{CheckError, CheckTarget, Finding, LlmUsage, Severity};
use crate::review::{ModelUsage, ReviewSummary};

use super::{RenderCheck, RenderCheckErrorRef, ReviewCounts, escape_html, escape_markdown};

#[cfg(test)]
use crate::llm::result::CheckResult;

pub fn render(result: &RenderCheck, repo: &str) -> String {
    let mut body = String::new();
    writeln!(body, "## Check: {}", escape_github_markdown(&result.check)).unwrap();
    writeln!(body).unwrap();
    writeln!(body, "- **Target:** {}", target(&result.target, repo)).unwrap();
    writeln!(body, "- **Status:** {}", result.status()).unwrap();
    writeln!(body).unwrap();
    if let Some(summary) = result.summary() {
        writeln!(body, "{}", escape_github_markdown(summary)).unwrap();
        writeln!(body).unwrap();
    }
    writeln!(body, "### Findings").unwrap();
    writeln!(body).unwrap();
    if result.findings().is_empty() {
        writeln!(body, "None.").unwrap();
    } else {
        for finding in result.findings() {
            writeln!(body, "- {}", render_finding(finding, repo)).unwrap();
        }
    }
    if let Some(error) = result.error() {
        writeln!(body).unwrap();
        writeln!(body, "> [!WARNING]").unwrap();
        writeln!(body, "> {}", escape_github_markdown(&display_error(error))).unwrap();
    }
    let iterations = result.iterations();
    let usage = result.usage();
    if iterations.is_some() || usage.is_some() {
        writeln!(body).unwrap();
        writeln!(body, "### Metadata").unwrap();
        writeln!(body).unwrap();
        if let Some(iterations) = iterations {
            writeln!(body, "- **Iterations:** {iterations}").unwrap();
        }
        if let Some(usage) = usage {
            write_usage_markdown(&mut body, "Check usage", usage);
        }
    }

    format!(
        "<details>\n<summary>Check: {} - Status: {} - Target: {}</summary>\n\n{}\n</details>",
        escape_github_html(&result.check),
        result.status(),
        escape_github_html(&display_target(&result.target)),
        body.trim_end()
    )
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
            escape_github_markdown(&usage.model),
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
                escape_github_markdown(model),
                usage.input_tokens,
                usage.output_tokens,
                usage.cost_usd,
            )
            .unwrap();
        }
    }
    output.trim_end().to_string()
}

fn render_finding(finding: &Finding, repo: &str) -> String {
    let commit = finding.commit.as_ref();
    let mut context = format!("[`{commit}`]({})", commit_url(repo, commit));
    if let Some(location) = &finding.location {
        let label = location_label(location.file.as_str(), location.line);
        write!(
            context,
            " · [{}]({})",
            escape_github_markdown(&label),
            file_url(repo, commit, &location.file, location.line)
        )
        .unwrap();
    }
    format!(
        "**{}** — {} ({context})",
        severity_name(finding.severity),
        escape_github_markdown(&finding.message)
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

fn location_label(file: &str, line: Option<u32>) -> String {
    match line {
        Some(line) => format!("{file}:{line}"),
        None => file.to_string(),
    }
}

fn write_usage_markdown(output: &mut String, heading: &str, usage: &LlmUsage) {
    writeln!(output).unwrap();
    writeln!(output, "### {heading}").unwrap();
    writeln!(output).unwrap();
    writeln!(output, "- **Input tokens:** {}", usage.input_tokens).unwrap();
    writeln!(output, "- **Output tokens:** {}", usage.output_tokens).unwrap();
    writeln!(output, "- **Cost:** ${:.6}", usage.cost_usd).unwrap();
    writeln!(
        output,
        "- **Model:** {}",
        escape_github_markdown(&usage.model)
    )
    .unwrap();
}

fn target(target: &CheckTarget, repo: &str) -> String {
    match target {
        CheckTarget::Commit(commit) => {
            let commit = commit.as_ref();
            format!("[`{commit}`]({})", commit_url(repo, commit))
        }
        CheckTarget::Range { from, to } => escape_github_markdown(&format!("{from}..{to}")),
    }
}

fn commit_url(repo: &str, commit: &str) -> String {
    format!("https://github.com/{repo}/commit/{commit}")
}

fn file_url(repo: &str, commit: &str, file: &str, line: Option<u32>) -> String {
    let mut url = format!(
        "https://github.com/{repo}/blob/{commit}/{}",
        encode_path(file)
    );
    if let Some(line) = line {
        write!(url, "#L{line}").unwrap();
    }
    url
}

fn encode_path(path: &str) -> String {
    path.split('/')
        .map(|segment| {
            segment
                .bytes()
                .map(|byte| match byte {
                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                        (byte as char).to_string()
                    }
                    _ => format!("%{byte:02X}"),
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn escape_github_markdown(value: &str) -> String {
    neutralize_mentions(&escape_markdown(value))
}

fn escape_github_html(value: &str) -> String {
    neutralize_mentions(&escape_html(value))
}

fn neutralize_mentions(value: &str) -> String {
    value.replace('@', "`@`")
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
    fn includes_commit_and_file_links() {
        let output = render(&result().into(), "owner/repo");

        assert!(output.contains("https://github.com/owner/repo/commit/abc1234"));
        assert!(output.contains("https://github.com/owner/repo/blob/abc1234/src/main.rs#L42"));
    }

    #[test]
    fn escapes_markdown_and_details_summary_content() {
        let mut result = result();
        result.check = "check </summary><script>".to_string();
        result.summary = "> quote\n- list".to_string();
        result.findings[0].message = "[link](url)".to_string();
        result.findings[0].location = Some(FileLocation {
            file: "src/]unsafe[.rs".to_string(),
            line: Some(7),
        });

        let output = render(&result.into(), "owner/repo");

        assert!(output.contains("<summary>Check: check &lt;/summary&gt;&lt;script&gt;"));
        assert!(output.contains("\\> quote \\- list"));
        assert!(output.contains("\\[link\\]\\(url\\)"));
        assert!(output.contains("[src/\\]unsafe\\[\\.rs:7]"));
        assert!(output.contains("src/%5Dunsafe%5B.rs#L7"));
        assert!(!output.contains("</summary><script>"));
    }

    #[test]
    fn neutralizes_mentions_in_dynamic_content() {
        let mut result = result();
        result.check = "@reviewers".to_string();
        result.summary = "Please notify @org/team".to_string();
        result.findings[0].message = "Assigned to @alice".to_string();

        let output = render(&result.into(), "owner/repo");

        assert!(output.contains("`@`reviewers"));
        assert!(output.contains("`@`org/team"));
        assert!(output.contains("`@`alice"));
        assert!(!output.contains("@reviewers"));
        assert!(!output.contains("@org/team"));
        assert!(!output.contains("@alice"));
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
}
