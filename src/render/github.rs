use std::collections::BTreeMap;
use std::fmt::Write;

use crate::llm::result::{CheckResult, CheckTarget, CheckUsage, Finding, Severity};
use crate::review::{ModelUsage, ReviewSummary};

use super::{escape_html, escape_markdown};

pub fn render(result: &CheckResult, repo: &str) -> String {
    let mut body = String::new();
    writeln!(body, "## Check: {}", escape_github_markdown(&result.check)).unwrap();
    writeln!(body).unwrap();
    writeln!(body, "- **Target:** {}", target(&result.target, repo)).unwrap();
    writeln!(body, "- **Status:** {}", status(result)).unwrap();
    writeln!(body).unwrap();
    writeln!(body, "{}", escape_github_markdown(&result.summary)).unwrap();
    writeln!(body).unwrap();
    writeln!(body, "### Findings").unwrap();
    writeln!(body).unwrap();
    if result.findings.is_empty() {
        writeln!(body, "None.").unwrap();
    } else {
        for finding in &result.findings {
            writeln!(body, "- {}", render_finding(finding, repo)).unwrap();
        }
    }
    if result.is_exhausted {
        writeln!(body).unwrap();
        writeln!(body, "> [!WARNING]").unwrap();
        writeln!(
            body,
            "> {}",
            escape_github_markdown(
                result
                    .exhaustion_reason
                    .as_deref()
                    .unwrap_or("check did not complete")
            )
        )
        .unwrap();
    }
    writeln!(body).unwrap();
    writeln!(body, "### Metadata").unwrap();
    writeln!(body).unwrap();
    writeln!(body, "- **Iterations:** {}", result.iterations).unwrap();
    write_usage_markdown(&mut body, &result.usage);

    format!(
        "<details>\n<summary>Check: {} - Status: {} - Target: {}</summary>\n\n{}\n</details>",
        escape_github_html(&result.check),
        status(result),
        escape_github_html(display_target(&result.target)),
        body.trim_end()
    )
}

pub fn render_review_summary(
    summary: &ReviewSummary,
    usage_by_model: &BTreeMap<String, ModelUsage>,
) -> String {
    let mut output = format!(
        "## Review summary\n\n- **Peer version:** {}\n- **Provider:** {}\n- **Model:** {}\n\n### Total token usage\n\n",
        summary.peer_version, summary.provider, summary.model,
    );
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

fn location_label(file: &str, line: Option<u32>) -> String {
    match line {
        Some(line) => format!("{file}:{line}"),
        None => file.to_string(),
    }
}

fn write_usage_markdown(output: &mut String, usage: &CheckUsage) {
    writeln!(output).unwrap();
    writeln!(output, "### Usage").unwrap();
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
        CheckTarget::Range(range) => escape_github_markdown(range),
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
    fn includes_commit_and_file_links() {
        let output = render(&result(), "owner/repo");

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

        let output = render(&result, "owner/repo");

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

        let output = render(&result, "owner/repo");

        assert!(output.contains("`@`reviewers"));
        assert!(output.contains("`@`org/team"));
        assert!(output.contains("`@`alice"));
        assert!(!output.contains("@reviewers"));
        assert!(!output.contains("@org/team"));
        assert!(!output.contains("@alice"));
    }
}
