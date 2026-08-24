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
    RenderDocument, RenderFinding, RenderStage, RenderStageErrorRef, ReviewCounts,
    clarification_message, escape_html, escape_markdown, join_review_sections, review_counts,
    usage_by_model,
};

pub fn render(document: &RenderDocument, repo: &str) -> String {
    let stages = document
        .stages
        .iter()
        .map(|stage| render_stage(stage, repo))
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
    let questions = render_questions(&document.questions, repo);
    let recommendations = render_recommendations(&document.recommendations, repo);
    let findings = render_findings(&document.findings, repo);
    join_review_sections(
        summary
            .into_iter()
            .chain([questions, recommendations, findings])
            .chain(context)
            .chain([stages]),
    )
}

fn render_stage(result: &RenderStage, repo: &str) -> String {
    let mut body = String::new();
    writeln!(body, "## Stage: {}", escape_github_markdown(&result.stage)).unwrap();
    writeln!(body).unwrap();
    writeln!(body, "- **Target:** {}", target(&result.target, repo)).unwrap();
    writeln!(body, "- **Status:** {}", result.status()).unwrap();
    writeln!(body).unwrap();
    if let Some(summary) = result.summary() {
        writeln!(body, "{}", escape_github_markdown(summary)).unwrap();
        writeln!(body).unwrap();
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
            write_usage_markdown(&mut body, "Stage usage", usage);
        }
    }

    format!(
        "<details>\n<summary>Stage: {} - Status: {} - Target: {}</summary>\n\n{}\n</details>",
        escape_github_html(&result.stage),
        result.status(),
        escape_github_html(&display_target(&result.target)),
        body.trim_end()
    )
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

fn render_questions(questions: &[KnowledgeQuestion], repo: &str) -> String {
    if questions.is_empty() {
        return String::new();
    }
    let mut output = "## Review questions\n".to_string();
    for question in questions {
        writeln!(
            output,
            "- **question/{}** — {} Evidence: {} Why it matters: {} ({})",
            question.category.as_str(),
            escape_github_markdown(&question.question),
            escape_github_markdown(&question.evidence),
            escape_github_markdown(&question.why_it_matters),
            related_context(&question.related_commits, question.location.as_ref(), repo,),
        )
        .unwrap();
    }
    output.trim_end().to_string()
}

fn render_recommendations(recommendations: &[StructuralRecommendation], repo: &str) -> String {
    if recommendations.is_empty() {
        return String::new();
    }
    let mut output = "## Structural recommendations\n".to_string();
    for recommendation in recommendations {
        writeln!(
            output,
            "- **recommendation/{}** — {} Rationale: {} ({})",
            recommendation.kind.as_str(),
            escape_github_markdown(&recommendation.message),
            escape_github_markdown(&recommendation.rationale),
            related_context(&recommendation.related_commits, None, repo),
        )
        .unwrap();
    }
    output.trim_end().to_string()
}

fn render_findings(findings: &[RenderFinding], repo: &str) -> String {
    if findings.is_empty() {
        return String::new();
    }
    let mut output = "## Review findings\n".to_string();
    for finding in findings {
        write!(
            output,
            "- **finding/{}** — {}",
            severity_name(finding.severity),
            escape_github_markdown(&finding.message),
        )
        .unwrap();
        if let Some(security) = &finding.security {
            write!(
                output,
                " Attacker control: {} Sensitive operation: {} Impact: {}",
                escape_github_markdown(&security.attacker_control),
                escape_github_markdown(&security.sensitive_operation),
                escape_github_markdown(&security.impact),
            )
            .unwrap();
        }
        writeln!(
            output,
            " ({})",
            finding_context(&finding.commit, finding.location.as_ref(), repo)
        )
        .unwrap();
    }
    output.trim_end().to_string()
}

fn related_context(
    commits: &[CommitHash],
    location: Option<&crate::stage::KnowledgeLocation>,
    repo: &str,
) -> String {
    commits
        .iter()
        .map(|commit| {
            let file = location
                .filter(|location| location.commit.matches(commit))
                .map(|location| &location.file);
            finding_context(commit, file, repo)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn finding_context(commit: &CommitHash, location: Option<&FileLocation>, repo: &str) -> String {
    let commit = commit.as_ref();
    let mut context = format!("[`{commit}`]({})", commit_url(repo, commit));
    if let Some(location) = location {
        let label = location_label(location.file.as_str(), location.line);
        write!(
            context,
            " · [{}]({})",
            escape_github_markdown(&label),
            file_url(repo, commit, &location.file, location.line)
        )
        .unwrap();
    }
    context
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
    writeln!(
        output,
        "- **Model:** {}",
        escape_github_markdown(&usage.model)
    )
    .unwrap();
}

fn target(target: &StageTarget, repo: &str) -> String {
    match target {
        StageTarget::Commit(commit) => {
            let commit = commit.as_ref();
            format!("[`{commit}`]({})", commit_url(repo, commit))
        }
        StageTarget::Range { from, to } => escape_github_markdown(&format!("{from}..{to}")),
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
    fn includes_commit_and_file_links() {
        let rendered = crate::render::RenderStageParts::from(result());
        let output = render_findings(&rendered.findings, "owner/repo");

        assert!(output.contains("https://github.com/owner/repo/commit/abc1234"));
        assert!(output.contains("https://github.com/owner/repo/blob/abc1234/src/main.rs#L42"));
    }

    #[test]
    fn renders_finding_without_reformatting_it() {
        let findings = vec![Finding {
            commit: CommitHash::new("abc1234").unwrap(),
            severity: Severity::High,
            message: "High-risk finding.".to_string(),
            location: Some(FileLocation {
                file: "src/main.rs".to_string(),
                line: Some(42),
            }),
        }];

        let findings = findings
            .into_iter()
            .map(crate::render::render_finding)
            .collect::<Vec<_>>();
        let output = render_findings(&findings, "owner/repo");

        assert!(output.contains(r"**finding/high** — High\-risk finding\."));
        assert!(!output.contains("**finding/high** — **high**"));
        assert!(output.contains("https://github.com/owner/repo/blob/abc1234/src/main.rs#L42"));
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
        let output = render_findings(&findings, "owner/repo");

        assert!(output.starts_with("## Review findings\n- "));
        assert!(!output.contains("\n\n- "));
    }

    #[test]
    fn includes_cache_usage_when_available() {
        let mut result = result();
        result.usage.cache_read_tokens = 80;
        result.usage.cache_write_tokens = 10;

        let rendered = crate::render::RenderStageParts::from(result);
        let output = render_stage(&rendered.stage, "owner/repo");

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

        assert!(!render_stage(&stage, "owner/repo").contains("### Metadata"));
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

        assert!(render_stage(&stage, "owner/repo").contains("### Metadata"));
    }

    #[test]
    fn escapes_markdown_and_details_summary_content() {
        let mut result = result();
        result.stage = "stage </summary><script>".to_string();
        result.summary = "> quote\n- list".to_string();
        result.findings[0].message = "[link](url)".to_string();
        result.findings[0].location = Some(FileLocation {
            file: "src/]unsafe[.rs".to_string(),
            line: Some(7),
        });

        let rendered = crate::render::RenderStageParts::from(result);
        let stage_output = render_stage(&rendered.stage, "owner/repo");
        let findings_output = render_findings(&rendered.findings, "owner/repo");

        assert!(stage_output.contains("<summary>Stage: stage &lt;/summary&gt;&lt;script&gt;"));
        assert!(stage_output.contains("\\> quote \\- list"));
        assert!(findings_output.contains("\\[link\\]\\(url\\)"));
        assert!(findings_output.contains("[src/\\]unsafe\\[\\.rs:7]"));
        assert!(findings_output.contains("src/%5Dunsafe%5B.rs#L7"));
        assert!(!stage_output.contains("</summary><script>"));
    }

    #[test]
    fn neutralizes_mentions_in_dynamic_content() {
        let mut result = result();
        result.stage = "@reviewers".to_string();
        result.summary = "Please notify @org/team".to_string();
        result.findings[0].message = "Assigned to @alice".to_string();

        let rendered = crate::render::RenderStageParts::from(result);
        let stage_output = render_stage(&rendered.stage, "owner/repo");
        let findings_output = render_findings(&rendered.findings, "owner/repo");

        assert!(stage_output.contains("`@`reviewers"));
        assert!(stage_output.contains("`@`org/team"));
        assert!(findings_output.contains("`@`alice"));
        assert!(!stage_output.contains("@reviewers"));
        assert!(!stage_output.contains("@org/team"));
        assert!(!findings_output.contains("@alice"));
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
}
