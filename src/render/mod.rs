use std::fmt;
use std::fmt::Write;
use std::io::IsTerminal;

use crate::cli::OutputFormat;
use crate::console::Console;
use crate::llm::checks::{CheckCommandErrorOutput, CheckCommandOutput, ErrorCode};
use crate::llm::result::{CheckOutcome, CheckResult, CheckTarget, Finding, Severity};
use crate::review::ReviewResult;
use owo_colors::Style;

pub fn render(input: &str, format: OutputFormat, console: Console) -> Result<String, RenderError> {
    render_impl(input, format, console, std::io::stdout().is_terminal())
}

fn render_impl(
    input: &str,
    format: OutputFormat,
    console: Console,
    use_color: bool,
) -> Result<String, RenderError> {
    let envelope: CheckCommandOutput =
        serde_json::from_str(input).map_err(RenderError::InvalidEnvelope)?;

    render_check_output_impl(&envelope, format, console, use_color)
}

fn render_check_output_impl(
    output: &CheckCommandOutput,
    format: OutputFormat,
    console: Console,
    use_color: bool,
) -> Result<String, RenderError> {
    log_usage(output, console);

    match format {
        OutputFormat::Json => render_json(output),
        OutputFormat::Terminal => Ok(render_terminal(output, use_color)),
        OutputFormat::Markdown => Ok(render_markdown(output)),
    }
}

pub fn render_check_output(
    output: &CheckCommandOutput,
    format: OutputFormat,
    console: Console,
) -> Result<String, RenderError> {
    render_check_output_impl(output, format, console, std::io::stdout().is_terminal())
}

pub fn render_review_result(
    result: &ReviewResult,
    format: OutputFormat,
    console: Console,
) -> Result<String, RenderError> {
    render_review_result_impl(result, format, console, std::io::stdout().is_terminal())
}

fn render_check_result_impl(
    result: &CheckResult,
    format: OutputFormat,
    console: Console,
    use_color: bool,
) -> Result<String, RenderError> {
    match format {
        OutputFormat::Json => render_check_output_impl(
            &CheckCommandOutput::success(result.clone()),
            OutputFormat::Json,
            console,
            use_color,
        ),
        OutputFormat::Terminal => {
            log_result_usage(result, console);
            Ok(render_terminal_result(result, use_color))
        }
        OutputFormat::Markdown => {
            log_result_usage(result, console);
            Ok(render_markdown_result(result))
        }
    }
}

fn render_review_result_impl(
    result: &ReviewResult,
    format: OutputFormat,
    console: Console,
    use_color: bool,
) -> Result<String, RenderError> {
    match format {
        OutputFormat::Json => render_review_json(result, console),
        OutputFormat::Terminal | OutputFormat::Markdown => result
            .outcomes
            .iter()
            .map(|outcome| render_check_outcome_impl(outcome, format, console, use_color))
            .collect::<Result<Vec<_>, _>>()
            .map(|rendered| rendered.join("\n\n")),
    }
}

fn render_check_outcome_impl(
    outcome: &CheckOutcome,
    format: OutputFormat,
    console: Console,
    use_color: bool,
) -> Result<String, RenderError> {
    match outcome {
        CheckOutcome::Success { check } => {
            render_check_result_impl(check, format, console, use_color)
        }
        CheckOutcome::NeedsUserInfo { request } => Ok(match format {
            OutputFormat::Json => unreachable!("review json renders the full review result"),
            OutputFormat::Terminal => format!(
                "{} {}\n{} {}\n\n{}",
                terminal_label("Check:", use_color),
                styled(&request.check, Style::new().bold(), use_color),
                terminal_label("Target:", use_color),
                display_target(&request.target),
                request.questions.join("\n")
            ),
            OutputFormat::Markdown => {
                let questions = request
                    .questions
                    .iter()
                    .map(|question| format!("- {question}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                format!(
                    "## Check: {}\n\nNeeds user info:\n\n{}",
                    request.check, questions
                )
            }
        }),
    }
}

fn render_json(output: &CheckCommandOutput) -> Result<String, RenderError> {
    let mut value = serde_json::to_value(output).map_err(RenderError::Serialization)?;
    remove_usage(&mut value);
    serde_json::to_string_pretty(&value).map_err(RenderError::Serialization)
}

fn render_review_json(result: &ReviewResult, console: Console) -> Result<String, RenderError> {
    log_review_usage(result, console);

    let mut value = serde_json::to_value(result).map_err(RenderError::Serialization)?;
    remove_review_usage(&mut value);
    serde_json::to_string_pretty(&value).map_err(RenderError::Serialization)
}

fn remove_usage(value: &mut serde_json::Value) {
    let Some(outcome) = value
        .get_mut("data")
        .and_then(serde_json::Value::as_object_mut)
    else {
        return;
    };

    if let Some(check) = outcome
        .get_mut("check")
        .and_then(serde_json::Value::as_object_mut)
    {
        check.remove("usage");
    }
    if let Some(request) = outcome
        .get_mut("request")
        .and_then(serde_json::Value::as_object_mut)
    {
        request.remove("usage");
    }
}

fn remove_review_usage(value: &mut serde_json::Value) {
    let Some(checks) = value
        .get_mut("outcomes")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return;
    };

    for outcome in checks {
        let Some(outcome) = outcome.as_object_mut() else {
            continue;
        };
        if let Some(check) = outcome
            .get_mut("check")
            .and_then(serde_json::Value::as_object_mut)
        {
            check.remove("usage");
        }
        if let Some(request) = outcome
            .get_mut("request")
            .and_then(serde_json::Value::as_object_mut)
        {
            request.remove("usage");
        }
    }
}

fn log_usage(output: &CheckCommandOutput, console: Console) {
    if let Ok(result) = output.as_result() {
        log_result_usage(result, console);
    }
}

fn log_review_usage(result: &ReviewResult, console: Console) {
    for outcome in &result.outcomes {
        if let CheckOutcome::Success { check } = outcome {
            log_result_usage(check, console);
        }
    }
}

fn log_result_usage(result: &CheckResult, console: Console) {
    console.verbose(format_args!(
        "Usage: {} input, {} output, ${:.6} ({})",
        result.usage.input_tokens,
        result.usage.output_tokens,
        result.usage.cost_usd,
        result.usage.model
    ));
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
    write!(
        output,
        "{} {:.0}% | {} {}",
        terminal_label("Confidence:", use_color),
        result.confidence.as_f64() * 100.0,
        terminal_label("Iterations:", use_color),
        result.iterations
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
                "status": "success",
                "check": {
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
            }
        })
    }

    fn success_envelope_with_finding() -> Value {
        let mut envelope = success_envelope();
        let check = &mut envelope["data"]["check"];
        check["summary"] = json!("A critical issue was found.");
        check["findings"] = json!([{
            "commit": "abc1234",
            "severity": "critical",
            "message": "User input reaches a shell command.",
            "file": "src/main.rs",
            "line": 42
        }]);
        check["confidence"] = json!(0.85);
        check["iterations"] = json!(2);
        envelope
    }

    fn success_envelope_without_usage() -> Value {
        let mut envelope = success_envelope();
        envelope["data"]["check"]
            .as_object_mut()
            .unwrap()
            .remove("usage");
        envelope
    }

    fn success_result() -> CheckResult {
        serde_json::from_value(success_envelope()["data"]["check"].clone()).unwrap()
    }

    fn success_result_with_finding() -> CheckResult {
        serde_json::from_value(success_envelope_with_finding()["data"]["check"].clone()).unwrap()
    }

    fn success_review_result() -> ReviewResult {
        let mut size = success_result();
        size.check = "size".to_string();
        let mut intent = success_result_with_finding();
        intent.check = "intent".to_string();

        ReviewResult {
            outcomes: vec![CheckOutcome::success(size), CheckOutcome::success(intent)],
            errors: Default::default(),
        }
    }

    fn console() -> Console {
        Console::default()
    }

    #[test]
    fn renders_check_envelope_as_pretty_json() {
        let input = serde_json::to_string(&success_envelope()).unwrap();

        let rendered = render(&input, OutputFormat::Json, console()).unwrap();

        assert!(rendered.contains('\n'));
        assert_eq!(
            serde_json::from_str::<Value>(&rendered).unwrap(),
            success_envelope_without_usage()
        );
    }

    #[test]
    fn renders_successful_check_for_terminal() {
        let input = success_envelope_with_finding().to_string();

        let rendered = render_impl(&input, OutputFormat::Terminal, console(), false).unwrap();

        assert_eq!(
            rendered,
            "\
Check: size
Target: abc1234
Status: issue

A critical issue was found.

Findings:
- [critical] User input reaches a shell command. (abc1234 src/main.rs:42)

Confidence: 85% | Iterations: 2"
        );
    }

    #[test]
    fn omits_usage_from_terminal_output() {
        let input = success_envelope_with_finding().to_string();

        let rendered = render_impl(&input, OutputFormat::Terminal, console(), false).unwrap();

        assert!(!rendered.contains("Usage:"));
        assert!(!rendered.contains("test-model"));
    }

    #[test]
    fn renders_check_result_for_terminal() {
        let result = success_result_with_finding();

        let rendered =
            render_check_result_impl(&result, OutputFormat::Terminal, console(), false).unwrap();

        assert!(rendered.contains("Check: size"));
        assert!(rendered.contains("Status: issue"));
        assert!(rendered.contains("User input reaches a shell command."));
    }

    #[test]
    fn renders_check_result_as_pretty_json_envelope() {
        let result = success_result();

        let rendered =
            render_check_result_impl(&result, OutputFormat::Json, console(), false).unwrap();
        let value: Value = serde_json::from_str(&rendered).unwrap();

        assert_eq!(value, success_envelope_without_usage());
    }

    #[test]
    fn renders_review_result_as_single_json_document() {
        let result = success_review_result();

        let rendered = render_review_result(&result, OutputFormat::Json, console()).unwrap();
        let value: Value = serde_json::from_str(&rendered).unwrap();

        assert_eq!(value["outcomes"].as_array().unwrap().len(), 2);
        assert_eq!(value["outcomes"][0]["status"], "success");
        assert_eq!(value["outcomes"][0]["check"]["check"], "size");
        assert_eq!(value["outcomes"][1]["check"]["check"], "intent");
        assert!(value["outcomes"][0]["check"].get("usage").is_none());
        assert!(value["outcomes"][1]["check"].get("usage").is_none());
    }

    #[test]
    fn renders_review_result_for_terminal() {
        let result = success_review_result();

        let rendered =
            render_review_result_impl(&result, OutputFormat::Terminal, console(), false).unwrap();

        assert!(rendered.contains("Check: size"));
        assert!(rendered.contains("Check: intent"));
        assert!(rendered.contains("\n\nCheck: intent"));
        assert!(!rendered.contains("Usage:"));
    }

    #[test]
    fn renders_review_result_for_markdown() {
        let result = success_review_result();

        let rendered = render_review_result(&result, OutputFormat::Markdown, console()).unwrap();

        assert!(rendered.contains("## Check: size"));
        assert!(rendered.contains("## Check: intent"));
        assert!(rendered.contains("\n\n## Check: intent"));
        assert!(!rendered.contains("**Usage:**"));
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
            console(),
            false,
        )
        .unwrap();

        assert!(rendered.contains("Status: ok"));
        assert!(rendered.contains("Findings: none"));
    }

    #[test]
    fn renders_exhausted_check_warning_for_terminal() {
        let mut envelope = success_envelope();
        envelope["data"]["check"]["is_exhausted"] = json!(true);
        envelope["data"]["check"]["exhaustion_reason"] = json!("max_iterations");

        let rendered = render_impl(
            &envelope.to_string(),
            OutputFormat::Terminal,
            console(),
            false,
        )
        .unwrap();

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

        let rendered = render_impl(
            &envelope.to_string(),
            OutputFormat::Terminal,
            console(),
            false,
        )
        .unwrap();

        assert_eq!(rendered, "error: config_invalid — invalid config");
    }

    #[test]
    fn renders_successful_check_for_markdown() {
        let rendered = render(
            &success_envelope_with_finding().to_string(),
            OutputFormat::Markdown,
            console(),
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
- **Iterations:** 2"
        );
    }

    #[test]
    fn renders_check_result_for_markdown() {
        let result = success_result_with_finding();

        let rendered =
            render_check_result_impl(&result, OutputFormat::Markdown, console(), false).unwrap();

        assert!(rendered.contains("## Check: size"));
        assert!(rendered.contains("- **Status:** issue"));
        assert!(rendered.contains("**critical**"));
    }

    #[test]
    fn renders_exhausted_check_warning_for_markdown() {
        let mut envelope = success_envelope();
        envelope["data"]["check"]["is_exhausted"] = json!(true);
        envelope["data"]["check"]["exhaustion_reason"] = json!("max_iterations");

        let rendered = render(&envelope.to_string(), OutputFormat::Markdown, console()).unwrap();

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

        let rendered = render(&envelope.to_string(), OutputFormat::Markdown, console()).unwrap();

        assert_eq!(rendered, "> [!CAUTION]\n> `config_invalid`: invalid config");
    }

    #[test]
    fn rejects_malformed_json() {
        let error = render("{", OutputFormat::Json, console()).unwrap_err();

        assert!(matches!(error, RenderError::InvalidEnvelope(_)));
    }

    #[test]
    fn rejects_envelope_without_status() {
        let input = json!({
            "data": success_envelope()["data"]
        });

        let error = render(&input.to_string(), OutputFormat::Json, console()).unwrap_err();

        assert!(matches!(error, RenderError::InvalidEnvelope(_)));
    }

    #[test]
    fn rejects_invalid_check_envelope_payload() {
        let input = json!({
            "status": "success",
            "data": {}
        });

        let error = render(&input.to_string(), OutputFormat::Json, console()).unwrap_err();

        assert!(matches!(error, RenderError::InvalidEnvelope(_)));
    }
}
