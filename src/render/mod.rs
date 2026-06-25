use std::fmt;
use std::fmt::Write;
use std::io::IsTerminal;

use crate::cli::OutputFormat;
use crate::llm::checks::{CheckCommandErrorOutput, CheckCommandOutput, ErrorCode};
use crate::llm::result::{CheckResult, CheckTarget, Finding, Severity};
use owo_colors::Style;

pub fn render(input: &str, format: OutputFormat) -> Result<String, RenderError> {
    render_impl(input, format, std::io::stdout().is_terminal())
}

fn render_impl(input: &str, format: OutputFormat, use_color: bool) -> Result<String, RenderError> {
    let envelope: CheckCommandOutput =
        serde_json::from_str(input).map_err(RenderError::InvalidEnvelope)?;

    match format {
        OutputFormat::Json => {
            serde_json::to_string_pretty(&envelope).map_err(RenderError::Serialization)
        }
        OutputFormat::Terminal => Ok(render_terminal(&envelope, use_color)),
        OutputFormat::Markdown => Ok(render_markdown(&envelope)),
    }
}

fn render_terminal(output: &CheckCommandOutput, use_color: bool) -> String {
    match output.as_result() {
        Ok(result) => render_terminal_result(result, use_color),
        Err(error) => render_terminal_error(error, use_color),
    }
}

fn render_terminal_result(result: &CheckResult, use_color: bool) -> String {
    let mut output = String::new();
    writeln!(
        output,
        "{} {}",
        terminal_label("Check:", use_color),
        styled(&result.check, Style::new().bold(), use_color)
    )
    .unwrap();
    writeln!(
        output,
        "{} {}",
        terminal_label("Target:", use_color),
        display_target(&result.target)
    )
    .unwrap();
    writeln!(
        output,
        "{} {}",
        terminal_label("Status:", use_color),
        terminal_status(check_status(&result.findings), use_color)
    )
    .unwrap();
    writeln!(output).unwrap();
    writeln!(output, "{}", result.summary).unwrap();

    if result.findings.is_empty() {
        writeln!(output).unwrap();
        writeln!(
            output,
            "{} {}",
            terminal_label("Findings:", use_color),
            styled("none", Style::new().green(), use_color)
        )
        .unwrap();
    } else {
        writeln!(output).unwrap();
        writeln!(output, "{}", terminal_label("Findings:", use_color)).unwrap();
        for finding in &result.findings {
            writeln!(output, "- {}", display_terminal_finding(finding, use_color)).unwrap();
        }
    }

    if result.is_exhausted {
        writeln!(output).unwrap();
        writeln!(
            output,
            "{} agent loop exhausted ({})",
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
        "{} {:.0}% | {} {}",
        terminal_label("Confidence:", use_color),
        result.confidence.as_f64() * 100.0,
        terminal_label("Iterations:", use_color),
        result.iterations
    )
    .unwrap();
    write!(
        output,
        "{} {} input, {} output, ${:.6} ({})",
        terminal_label("Usage:", use_color),
        result.usage.input_tokens,
        result.usage.output_tokens,
        result.usage.cost_usd,
        result.usage.model
    )
    .unwrap();

    output
}

fn render_terminal_error(error: &CheckCommandErrorOutput, use_color: bool) -> String {
    format!(
        "{} {} — {}",
        styled("error:", Style::new().red().bold(), use_color),
        styled(
            error_code_name(error.code),
            Style::new().red().bold(),
            use_color
        ),
        error.message
    )
}

fn render_markdown(output: &CheckCommandOutput) -> String {
    match output.as_result() {
        Ok(result) => render_markdown_result(result),
        Err(error) => render_markdown_error(error),
    }
}

fn render_markdown_result(result: &CheckResult) -> String {
    let mut output = String::new();
    writeln!(output, "## Check: {}", result.check).unwrap();
    writeln!(output).unwrap();
    writeln!(output, "- **Target:** `{}`", display_target(&result.target)).unwrap();
    writeln!(output, "- **Status:** {}", check_status(&result.findings)).unwrap();
    writeln!(output).unwrap();
    writeln!(output, "{}", result.summary).unwrap();
    writeln!(output).unwrap();
    writeln!(output, "### Findings").unwrap();
    writeln!(output).unwrap();

    if result.findings.is_empty() {
        writeln!(output, "None.").unwrap();
    } else {
        for finding in &result.findings {
            writeln!(output, "- {}", display_markdown_finding(finding)).unwrap();
        }
    }

    if result.is_exhausted {
        writeln!(output).unwrap();
        writeln!(output, "> [!WARNING]").unwrap();
        writeln!(
            output,
            "> Agent loop exhausted: `{}`",
            result
                .exhaustion_reason
                .as_deref()
                .unwrap_or("unknown reason")
        )
        .unwrap();
    }

    writeln!(output).unwrap();
    writeln!(output, "### Metadata").unwrap();
    writeln!(output).unwrap();
    writeln!(
        output,
        "- **Confidence:** {:.0}%",
        result.confidence.as_f64() * 100.0
    )
    .unwrap();
    writeln!(output, "- **Iterations:** {}", result.iterations).unwrap();
    writeln!(
        output,
        "- **Usage:** {} input, {} output, ${:.6} ({})",
        result.usage.input_tokens,
        result.usage.output_tokens,
        result.usage.cost_usd,
        result.usage.model
    )
    .unwrap();

    output.trim_end().to_string()
}

fn render_markdown_error(error: &CheckCommandErrorOutput) -> String {
    format!(
        "> [!CAUTION]\n> `{}`: {}",
        error_code_name(error.code),
        error.message
    )
}

fn display_markdown_finding(finding: &Finding) -> String {
    let mut context = format!("`{}`", finding.commit);
    if let Some(location) = &finding.location {
        let location = if let Some(line) = location.line {
            format!("{}:{line}", location.file)
        } else {
            location.file.clone()
        };
        write!(context, " · `{location}`").unwrap();
    }

    format!(
        "**{}** — {} ({context})",
        severity_name(finding.severity),
        finding.message
    )
}

fn display_target(target: &CheckTarget) -> &str {
    match target {
        CheckTarget::Commit(commit) => commit.as_ref(),
        CheckTarget::Range(range) => range,
    }
}

fn check_status(findings: &[Finding]) -> &'static str {
    match findings.iter().map(|finding| finding.severity).max() {
        Some(Severity::Critical | Severity::High) => "issue",
        Some(Severity::Medium | Severity::Low) => "warning",
        Some(Severity::Info) | None => "ok",
    }
}

fn display_terminal_finding(finding: &Finding, use_color: bool) -> String {
    let location = finding.location.as_ref().map(|location| {
        if let Some(line) = location.line {
            format!("{}:{line}", location.file)
        } else {
            location.file.clone()
        }
    });
    let context = match location {
        Some(location) => format!("{} {location}", finding.commit),
        None => finding.commit.to_string(),
    };

    format!(
        "[{}] {} ({})",
        terminal_severity(finding.severity, use_color),
        finding.message,
        styled(context, Style::new().dimmed(), use_color)
    )
}

fn terminal_label(label: &str, use_color: bool) -> String {
    styled(label, Style::new().bright_blue().bold(), use_color)
}

fn terminal_status(status: &str, use_color: bool) -> String {
    let style = match status {
        "ok" => Style::new().green().bold(),
        "warning" => Style::new().yellow().bold(),
        "issue" => Style::new().red().bold(),
        _ => Style::new().bold(),
    };
    styled(status, style, use_color)
}

fn terminal_severity(severity: Severity, use_color: bool) -> String {
    let style = match severity {
        Severity::Info => Style::new().cyan(),
        Severity::Low => Style::new().blue(),
        Severity::Medium => Style::new().yellow(),
        Severity::High => Style::new().red(),
        Severity::Critical => Style::new().bright_red().bold(),
    };
    styled(severity_name(severity), style, use_color)
}

fn styled(value: impl fmt::Display, style: Style, use_color: bool) -> String {
    if use_color {
        style.style(value).to_string()
    } else {
        value.to_string()
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

fn error_code_name(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::ConfigInvalid => "config_invalid",
        ErrorCode::GitCommandFailed => "git_command_failed",
        ErrorCode::Internal => "internal",
        ErrorCode::InvalidArgument => "invalid_argument",
        ErrorCode::LlmRequestFailed => "llm_request_failed",
    }
}

#[derive(Debug)]
pub enum RenderError {
    InvalidEnvelope(serde_json::Error),
    Serialization(serde_json::Error),
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEnvelope(error) => write!(f, "invalid check envelope: {error}"),
            Self::Serialization(error) => write!(f, "cannot serialize envelope: {error}"),
        }
    }
}

impl std::error::Error for RenderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidEnvelope(err) => Some(err),
            Self::Serialization(err) => Some(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn success_envelope() -> Value {
        json!({
            "status": "success",
            "data": {
                "check": "size",
                "target": "abc1234",
                "summary": "The commit is appropriately sized.",
                "findings": [],
                "confidence": 0.9,
                "iterations": 1,
                "is_exhausted": false,
                "exhaustion_reason": null,
                "usage": {
                    "input_tokens": 100,
                    "output_tokens": 20,
                    "cost_usd": 0.001,
                    "model": "test-model"
                }
            }
        })
    }

    fn success_envelope_with_finding() -> Value {
        let mut envelope = success_envelope();
        envelope["data"]["summary"] = json!("A critical issue was found.");
        envelope["data"]["findings"] = json!([{
            "commit": "abc1234",
            "severity": "critical",
            "message": "User input reaches a shell command.",
            "file": "src/main.rs",
            "line": 42
        }]);
        envelope["data"]["confidence"] = json!(0.85);
        envelope["data"]["iterations"] = json!(2);
        envelope
    }

    #[test]
    fn renders_check_envelope_as_pretty_json() {
        let input = serde_json::to_string(&success_envelope()).unwrap();

        let rendered = render(&input, OutputFormat::Json).unwrap();

        assert!(rendered.contains('\n'));
        assert_eq!(
            serde_json::from_str::<Value>(&rendered).unwrap(),
            success_envelope()
        );
    }

    #[test]
    fn renders_successful_check_for_terminal() {
        let input = success_envelope_with_finding().to_string();

        let rendered = render_impl(&input, OutputFormat::Terminal, false).unwrap();

        assert_eq!(
            rendered,
            "\
Check: size
Target: abc1234
Status: issue

A critical issue was found.

Findings:
- [critical] User input reaches a shell command. (abc1234 src/main.rs:42)

Confidence: 85% | Iterations: 2
Usage: 100 input, 20 output, $0.001000 (test-model)"
        );
    }

    #[test]
    fn styles_terminal_output_when_color_is_enabled() {
        let envelope: CheckCommandOutput =
            serde_json::from_value(success_envelope_with_finding()).unwrap();

        let rendered = render_terminal(&envelope, true);

        assert!(rendered.contains("\u{1b}["));
        assert!(rendered.contains("Check:"));
        assert!(rendered.contains("issue"));
        assert!(rendered.contains("critical"));
    }

    #[test]
    fn omits_ansi_codes_when_color_is_disabled() {
        let envelope: CheckCommandOutput =
            serde_json::from_value(success_envelope_with_finding()).unwrap();

        let rendered = render_terminal(&envelope, false);

        assert!(!rendered.contains("\u{1b}["));
    }

    #[test]
    fn renders_check_without_findings_for_terminal() {
        let rendered = render_impl(
            &success_envelope().to_string(),
            OutputFormat::Terminal,
            false,
        )
        .unwrap();

        assert!(rendered.contains("Status: ok"));
        assert!(rendered.contains("Findings: none"));
    }

    #[test]
    fn renders_exhausted_check_warning_for_terminal() {
        let mut envelope = success_envelope();
        envelope["data"]["is_exhausted"] = json!(true);
        envelope["data"]["exhaustion_reason"] = json!("max_iterations");

        let rendered = render_impl(&envelope.to_string(), OutputFormat::Terminal, false).unwrap();

        assert!(rendered.contains("Warning: agent loop exhausted (max_iterations)"));
    }

    #[test]
    fn renders_check_error_for_terminal() {
        let envelope = json!({
            "status": "error",
            "error": {
                "code": "config_invalid",
                "message": "invalid config",
                "is_retryable": false
            }
        });

        let rendered = render_impl(&envelope.to_string(), OutputFormat::Terminal, false).unwrap();

        assert_eq!(rendered, "error: config_invalid — invalid config");
    }

    #[test]
    fn renders_successful_check_for_markdown() {
        let rendered = render(
            &success_envelope_with_finding().to_string(),
            OutputFormat::Markdown,
        )
        .unwrap();

        assert_eq!(
            rendered,
            "\
## Check: size

- **Target:** `abc1234`
- **Status:** issue

A critical issue was found.

### Findings

- **critical** — User input reaches a shell command. (`abc1234` · `src/main.rs:42`)

### Metadata

- **Confidence:** 85%
- **Iterations:** 2
- **Usage:** 100 input, 20 output, $0.001000 (test-model)"
        );
    }

    #[test]
    fn renders_exhausted_check_warning_for_markdown() {
        let mut envelope = success_envelope();
        envelope["data"]["is_exhausted"] = json!(true);
        envelope["data"]["exhaustion_reason"] = json!("max_iterations");

        let rendered = render(&envelope.to_string(), OutputFormat::Markdown).unwrap();

        assert!(rendered.contains("> [!WARNING]\n> Agent loop exhausted: `max_iterations`"));
    }

    #[test]
    fn renders_check_error_for_markdown() {
        let envelope = json!({
            "status": "error",
            "error": {
                "code": "config_invalid",
                "message": "invalid config",
                "is_retryable": false
            }
        });

        let rendered = render(&envelope.to_string(), OutputFormat::Markdown).unwrap();

        assert_eq!(rendered, "> [!CAUTION]\n> `config_invalid`: invalid config");
    }

    #[test]
    fn rejects_malformed_json() {
        let error = render("{", OutputFormat::Json).unwrap_err();

        assert!(matches!(error, RenderError::InvalidEnvelope(_)));
    }

    #[test]
    fn rejects_envelope_without_status() {
        let input = json!({
            "data": success_envelope()["data"]
        });

        let error = render(&input.to_string(), OutputFormat::Json).unwrap_err();

        assert!(matches!(error, RenderError::InvalidEnvelope(_)));
    }

    #[test]
    fn rejects_invalid_check_envelope_payload() {
        let input = json!({
            "status": "success",
            "data": {}
        });

        let error = render(&input.to_string(), OutputFormat::Json).unwrap_err();

        assert!(matches!(error, RenderError::InvalidEnvelope(_)));
    }
}
