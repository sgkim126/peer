use std::fmt;
use std::fmt::Write;
use std::io::IsTerminal;

use crate::cli::OutputFormat;
use crate::console::Console;
use crate::llm::checks::{CheckCommandErrorOutput, CheckCommandOutput, ErrorCode};
use crate::llm::result::{
    CheckOutcome, CheckResult, CheckTarget, CheckUserInfoRequest, Finding, Severity,
};
use crate::review::{ReviewCheck, ReviewCheckError, ReviewResult};
use owo_colors::Style;

#[derive(Clone, Debug)]
pub enum RenderOptions {
    Json,
    Terminal,
    Markdown,
    Github {
        #[allow(dead_code)]
        repo: String,
    },
}

impl RenderOptions {
    pub fn from_cli(
        format: OutputFormat,
        github_repo: Option<String>,
    ) -> Result<Self, RenderOptionsError> {
        match (format, github_repo) {
            (OutputFormat::Json, None) => Ok(Self::Json),
            (OutputFormat::Terminal, None) => Ok(Self::Terminal),
            (OutputFormat::Markdown, None) => Ok(Self::Markdown),
            (OutputFormat::Github, Some(repo)) => {
                validate_github_repo(&repo)?;
                Ok(Self::Github { repo })
            }
            (OutputFormat::Github, None) => Err(RenderOptionsError::MissingGithubRepo),
            (_, Some(_)) => Err(RenderOptionsError::UnexpectedGithubRepo),
        }
    }
}

fn validate_github_repo(repo: &str) -> Result<(), RenderOptionsError> {
    let Some((owner, name)) = repo.split_once('/') else {
        return Err(RenderOptionsError::InvalidGithubRepo);
    };
    if owner.is_empty() || name.is_empty() || name.contains('/') {
        return Err(RenderOptionsError::InvalidGithubRepo);
    }
    if !owner.chars().all(is_github_repo_part_char) || !name.chars().all(is_github_repo_part_char) {
        return Err(RenderOptionsError::InvalidGithubRepoCharacters);
    }
    Ok(())
}

fn is_github_repo_part_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-')
}

#[allow(dead_code)]
fn github_commit_url(repo: &str, commit: &str) -> String {
    format!("https://github.com/{repo}/commit/{commit}")
}

#[allow(dead_code)]
fn github_file_url(repo: &str, commit: &str, file: &str, line: Option<u32>) -> String {
    let file = encode_github_path(file);
    let mut url = format!("https://github.com/{repo}/blob/{commit}/{file}");
    if let Some(line) = line {
        write!(url, "#L{line}").unwrap();
    }
    url
}

fn encode_github_path(path: &str) -> String {
    path.split('/')
        .map(encode_github_path_segment)
        .collect::<Vec<_>>()
        .join("/")
}

fn encode_github_path_segment(segment: &str) -> String {
    let mut encoded = String::new();
    for byte in segment.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char)
            }
            _ => write!(encoded, "%{byte:02X}").unwrap(),
        }
    }
    encoded
}

pub fn render(
    input: &str,
    options: RenderOptions,
    console: Console,
) -> Result<String, RenderError> {
    render_impl(input, &options, console, std::io::stdout().is_terminal())
}

fn render_impl(
    input: &str,
    options: &RenderOptions,
    console: Console,
    use_color: bool,
) -> Result<String, RenderError> {
    let envelope: CheckCommandOutput =
        serde_json::from_str(input).map_err(RenderError::InvalidEnvelope)?;

    render_check_output_impl(&envelope, options, console, use_color)
}

fn render_check_output_impl(
    output: &CheckCommandOutput,
    options: &RenderOptions,
    console: Console,
    use_color: bool,
) -> Result<String, RenderError> {
    log_usage(output, console);

    match options {
        RenderOptions::Json => render_json(output),
        RenderOptions::Terminal => Ok(render_terminal(output, use_color)),
        RenderOptions::Markdown => Ok(render_markdown(output)),
        RenderOptions::Github { repo } => Ok(render_github(output, repo)),
    }
}

pub fn render_check_output(
    output: &CheckCommandOutput,
    options: RenderOptions,
    console: Console,
) -> Result<String, RenderError> {
    render_check_output_impl(output, &options, console, std::io::stdout().is_terminal())
}

pub fn render_review_result(
    result: &ReviewResult,
    options: RenderOptions,
    console: Console,
) -> Result<String, RenderError> {
    render_review_result_impl(result, &options, console, std::io::stdout().is_terminal())
}

fn render_check_result_impl(
    result: &CheckResult,
    options: &RenderOptions,
    console: Console,
    use_color: bool,
) -> Result<String, RenderError> {
    match options {
        RenderOptions::Json => render_check_output_impl(
            &CheckCommandOutput::success(result.clone()),
            &RenderOptions::Json,
            console,
            use_color,
        ),
        RenderOptions::Terminal => {
            log_result_usage(result, console);
            Ok(render_terminal_result(result, use_color))
        }
        RenderOptions::Markdown => {
            log_result_usage(result, console);
            Ok(render_markdown_result(result))
        }
        RenderOptions::Github { repo } => {
            log_result_usage(result, console);
            Ok(render_github_result(result, repo))
        }
    }
}

fn render_review_result_impl(
    result: &ReviewResult,
    options: &RenderOptions,
    console: Console,
    use_color: bool,
) -> Result<String, RenderError> {
    match options {
        RenderOptions::Json => render_review_json(result, console),
        RenderOptions::Terminal | RenderOptions::Markdown | RenderOptions::Github { .. } => {
            let outcomes = result
                .outcomes
                .iter()
                .map(|outcome| render_check_outcome_impl(outcome, options, console, use_color));
            let errors = result
                .errors
                .iter()
                .map(|error| Ok(render_review_check_error(error, options, use_color)));

            outcomes
                .chain(errors)
                .collect::<Result<Vec<_>, _>>()
                .map(|rendered| rendered.join("\n\n"))
        }
    }
}

fn render_check_outcome_impl(
    outcome: &CheckOutcome,
    options: &RenderOptions,
    console: Console,
    use_color: bool,
) -> Result<String, RenderError> {
    match outcome {
        CheckOutcome::Success { check } => {
            render_check_result_impl(check, options, console, use_color)
        }
        CheckOutcome::NeedsUserInfo { request } => Ok(match options {
            RenderOptions::Json => unreachable!("review json renders the full review result"),
            RenderOptions::Terminal => render_terminal_user_info_request(request, use_color),
            RenderOptions::Markdown => render_markdown_user_info_request(request),
            RenderOptions::Github { .. } => render_github_user_info_request(request),
        }),
    }
}

fn render_terminal_user_info_request(request: &CheckUserInfoRequest, use_color: bool) -> String {
    format!(
        "{} {}\n{} {}\n{} {}\n\n{}",
        terminal_label("Check:", use_color),
        styled(&request.check, Style::new().bold(), use_color),
        terminal_label("Target:", use_color),
        display_target(&request.target),
        terminal_label("Status:", use_color),
        styled("needs_user_info", Style::new().yellow().bold(), use_color),
        request.questions.join("\n")
    )
}

fn render_markdown_user_info_request(request: &CheckUserInfoRequest) -> String {
    let questions = request
        .questions
        .iter()
        .map(|question| format!("- {question}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "## Check: {}\n\n- **Target:** `{}`\n- **Status:** needs_user_info\n\n### Questions\n\n{}",
        request.check,
        display_target(&request.target),
        questions
    )
}

fn render_github_user_info_request(request: &CheckUserInfoRequest) -> String {
    let rendered = render_markdown_user_info_request(request);
    format!(
        "<details>\n<summary>Check: {} - Status: needs_user_info - Target: {}</summary>\n\n{}\n</details>",
        request.check,
        display_target(&request.target),
        rendered
    )
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
    if let Ok(CheckOutcome::Success { check }) = output.as_outcome() {
        log_result_usage(check, console);
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
    match output.as_outcome() {
        Ok(outcome) => {
            render_check_outcome_for_command(outcome, CommandRenderFormat::Terminal { use_color })
        }
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
    match output.as_outcome() {
        Ok(outcome) => render_check_outcome_for_command(outcome, CommandRenderFormat::Markdown),
        Err(error) => render_markdown_error(error),
    }
}

fn render_github(output: &CheckCommandOutput, repo: &str) -> String {
    match output.as_outcome() {
        Ok(CheckOutcome::Success { check }) => render_github_result(check, repo),
        Ok(CheckOutcome::NeedsUserInfo { request }) => render_github_user_info_request(request),
        Err(error) => render_markdown_error(error),
    }
}

enum CommandRenderFormat {
    Terminal { use_color: bool },
    Markdown,
}

fn render_check_outcome_for_command(outcome: &CheckOutcome, format: CommandRenderFormat) -> String {
    match outcome {
        CheckOutcome::Success { check } => match format {
            CommandRenderFormat::Terminal { use_color } => render_terminal_result(check, use_color),
            CommandRenderFormat::Markdown => render_markdown_result(check),
        },
        CheckOutcome::NeedsUserInfo { request } => match format {
            CommandRenderFormat::Terminal { use_color } => {
                render_terminal_user_info_request(request, use_color)
            }
            CommandRenderFormat::Markdown => render_markdown_user_info_request(request),
        },
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

fn render_github_result(result: &CheckResult, repo: &str) -> String {
    let mut output = String::new();
    writeln!(output, "## Check: {}", result.check).unwrap();
    writeln!(output).unwrap();
    writeln!(
        output,
        "- **Target:** {}",
        display_github_target(&result.target, repo)
    )
    .unwrap();
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
            writeln!(output, "- {}", display_github_finding(finding, repo)).unwrap();
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

    format!(
        "<details>\n<summary>Check: {} - Status: {} - Target: {}</summary>\n\n{}\n</details>",
        result.check,
        check_status(&result.findings),
        display_target(&result.target),
        output.trim_end()
    )
}

fn render_markdown_error(error: &CheckCommandErrorOutput) -> String {
    format!(
        "> [!CAUTION]\n> `{}`: {}",
        error_code_name(error.code),
        error.message
    )
}

fn render_review_check_error(
    review_error: &ReviewCheckError,
    options: &RenderOptions,
    use_color: bool,
) -> String {
    let (check, target) = review_check_name_and_target(&review_error.check);
    let error = CheckCommandErrorOutput::from_ref(&review_error.error);

    match options {
        RenderOptions::Terminal => format!(
            "{} {}\n{} {}\n{} {}\n\n{} {} — {}",
            terminal_label("Check:", use_color),
            styled(check, Style::new().bold(), use_color),
            terminal_label("Target:", use_color),
            target,
            terminal_label("Status:", use_color),
            styled("failed", Style::new().red().bold(), use_color),
            terminal_label("Error:", use_color),
            styled(
                error_code_name(error.code),
                Style::new().red().bold(),
                use_color
            ),
            error.message
        ),
        RenderOptions::Markdown => render_markdown_review_check_error(check, target, &error),
        RenderOptions::Github { .. } => format!(
            "<details>\n<summary>Check: {check} - Status: failed - Target: {target}</summary>\n\n{}\n</details>",
            render_markdown_review_check_error(check, target, &error)
        ),
        RenderOptions::Json => unreachable!("review json is rendered separately"),
    }
}

fn render_markdown_review_check_error(
    check: &str,
    target: &str,
    error: &CheckCommandErrorOutput,
) -> String {
    format!(
        "## Check: {check}\n\n- **Target:** `{target}`\n- **Status:** failed\n\n> [!CAUTION]\n> `{}`: {}",
        error_code_name(error.code),
        error.message
    )
}

fn review_check_name_and_target(check: &ReviewCheck) -> (&str, &str) {
    match check {
        ReviewCheck::Size { revision } => ("size", revision),
        ReviewCheck::Intent { revision } => ("intent", revision),
        ReviewCheck::Quality { revision } => ("quality", revision),
        ReviewCheck::Security { revision } => ("security", revision),
        ReviewCheck::Coherence { range } => ("coherence", range),
    }
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

fn display_github_target(target: &CheckTarget, repo: &str) -> String {
    match target {
        CheckTarget::Commit(commit) => {
            let commit = commit.as_ref();
            format!("[`{commit}`]({})", github_commit_url(repo, commit))
        }
        CheckTarget::Range(range) => format!("`{range}`"),
    }
}

fn display_github_finding(finding: &Finding, repo: &str) -> String {
    let commit = finding.commit.as_ref();
    let mut context = format!("[`{commit}`]({})", github_commit_url(repo, commit));
    if let Some(location) = &finding.location {
        let label = if let Some(line) = location.line {
            format!("{}:{line}", location.file)
        } else {
            location.file.clone()
        };
        let url = github_file_url(repo, commit, &location.file, location.line);
        write!(context, " · [`{label}`]({url})").unwrap();
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

#[derive(Debug, PartialEq, Eq)]
pub enum RenderOptionsError {
    MissingGithubRepo,
    UnexpectedGithubRepo,
    InvalidGithubRepo,
    InvalidGithubRepoCharacters,
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEnvelope(error) => write!(f, "invalid check envelope: {error}"),
            Self::Serialization(error) => write!(f, "cannot serialize envelope: {error}"),
        }
    }
}

impl fmt::Display for RenderOptionsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingGithubRepo => write!(f, "--format github requires --repo <owner/name>"),
            Self::UnexpectedGithubRepo => {
                write!(f, "--repo can only be used with --format github")
            }
            Self::InvalidGithubRepo => write!(f, "--repo must use the form owner/name"),
            Self::InvalidGithubRepoCharacters => write!(
                f,
                "--repo may only contain ASCII letters, digits, '.', '_', and '-'"
            ),
        }
    }
}

impl std::error::Error for RenderOptionsError {}

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
    use crate::error::PeerError;
    use crate::llm::checks::CheckCommandError;

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

    fn needs_user_info_envelope() -> Value {
        json!({
            "status": "success",
            "data": {
                "status": "needs_user_info",
                "request": {
                    "check": "security",
                    "target": "abc1234",
                    "questions": [
                        "Which production auth policy applies here, and why does it affect this security check?",
                        "Is this endpoint exposed publicly, and why is that needed to assess exploitability?"
                    ],
                    "iterations": 1,
                    "usage": {
                        "input_tokens": 120,
                        "output_tokens": 30,
                        "cost_usd": 0.002,
                        "model": "test-model"
                    }
                }
            }
        })
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

    fn needs_user_info_outcome() -> CheckOutcome {
        serde_json::from_value(needs_user_info_envelope()["data"].clone()).unwrap()
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

    fn mixed_review_result() -> ReviewResult {
        let mut result = success_review_result();
        result.outcomes.push(needs_user_info_outcome());
        result
    }

    fn review_result_with_failed_check() -> ReviewResult {
        ReviewResult {
            outcomes: vec![],
            errors: vec![ReviewCheckError {
                check: ReviewCheck::Security {
                    revision: "abc1234".to_string(),
                },
                error: CheckCommandError::Config(PeerError::InvalidConfig {
                    message: "missing API key".to_string(),
                    source: None,
                }),
            }],
        }
    }

    fn console() -> Console {
        Console::default()
    }

    #[test]
    fn builds_render_options_from_cli_arguments() {
        assert!(matches!(
            RenderOptions::from_cli(OutputFormat::Json, None).unwrap(),
            RenderOptions::Json
        ));
        assert!(matches!(
            RenderOptions::from_cli(OutputFormat::Terminal, None).unwrap(),
            RenderOptions::Terminal
        ));
        assert!(matches!(
            RenderOptions::from_cli(OutputFormat::Markdown, None).unwrap(),
            RenderOptions::Markdown
        ));
        assert!(matches!(
            RenderOptions::from_cli(OutputFormat::Github, Some("sgkim126/peer".into())).unwrap(),
            RenderOptions::Github { .. }
        ));
    }

    #[test]
    fn rejects_invalid_render_options_from_cli_arguments() {
        assert_eq!(
            RenderOptions::from_cli(OutputFormat::Github, None).unwrap_err(),
            RenderOptionsError::MissingGithubRepo
        );
        assert_eq!(
            RenderOptions::from_cli(OutputFormat::Markdown, Some("sgkim126/peer".into()))
                .unwrap_err(),
            RenderOptionsError::UnexpectedGithubRepo
        );
        assert_eq!(
            RenderOptions::from_cli(OutputFormat::Github, Some("sgkim126".into())).unwrap_err(),
            RenderOptionsError::InvalidGithubRepo
        );
        assert_eq!(
            RenderOptions::from_cli(OutputFormat::Github, Some("sgkim126/peer/extra".into()))
                .unwrap_err(),
            RenderOptionsError::InvalidGithubRepo
        );
    }

    #[test]
    fn builds_github_commit_url() {
        assert_eq!(
            github_commit_url("sgkim126/peer", "abc1234"),
            "https://github.com/sgkim126/peer/commit/abc1234"
        );
    }

    #[test]
    fn builds_github_file_url_with_line() {
        assert_eq!(
            github_file_url("sgkim126/peer", "abc1234", "src/main.rs", Some(42)),
            "https://github.com/sgkim126/peer/blob/abc1234/src/main.rs#L42"
        );
    }

    #[test]
    fn builds_github_file_url_without_line() {
        assert_eq!(
            github_file_url("sgkim126/peer", "abc1234", "src/main.rs", None),
            "https://github.com/sgkim126/peer/blob/abc1234/src/main.rs"
        );
    }

    #[test]
    fn encodes_github_file_path_segments() {
        assert_eq!(
            github_file_url("sgkim126/peer", "abc1234", "docs/hello world#1.md", Some(7)),
            "https://github.com/sgkim126/peer/blob/abc1234/docs/hello%20world%231.md#L7"
        );
    }

    #[test]
    fn renders_check_envelope_as_pretty_json() {
        let input = serde_json::to_string(&success_envelope()).unwrap();

        let rendered = render(&input, RenderOptions::Json, console()).unwrap();

        assert!(rendered.contains('\n'));
        assert_eq!(
            serde_json::from_str::<Value>(&rendered).unwrap(),
            success_envelope_without_usage()
        );
    }

    #[test]
    fn renders_successful_check_for_terminal() {
        let input = success_envelope_with_finding().to_string();

        let rendered = render_impl(&input, &RenderOptions::Terminal, console(), false).unwrap();

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
    fn renders_user_info_request_check_for_terminal() {
        let input = needs_user_info_envelope().to_string();

        let rendered = render_impl(&input, &RenderOptions::Terminal, console(), false).unwrap();

        assert_eq!(
            rendered,
            "\
Check: security
Target: abc1234
Status: needs_user_info

Which production auth policy applies here, and why does it affect this security check?
Is this endpoint exposed publicly, and why is that needed to assess exploitability?"
        );
    }

    #[test]
    fn omits_usage_from_terminal_output() {
        let input = success_envelope_with_finding().to_string();

        let rendered = render_impl(&input, &RenderOptions::Terminal, console(), false).unwrap();

        assert!(!rendered.contains("Usage:"));
        assert!(!rendered.contains("test-model"));
    }

    #[test]
    fn renders_check_result_for_terminal() {
        let result = success_result_with_finding();

        let rendered =
            render_check_result_impl(&result, &RenderOptions::Terminal, console(), false).unwrap();

        assert!(rendered.contains("Check: size"));
        assert!(rendered.contains("Status: issue"));
        assert!(rendered.contains("User input reaches a shell command."));
    }

    #[test]
    fn renders_check_result_as_pretty_json_envelope() {
        let result = success_result();

        let rendered =
            render_check_result_impl(&result, &RenderOptions::Json, console(), false).unwrap();
        let value: Value = serde_json::from_str(&rendered).unwrap();

        assert_eq!(value, success_envelope_without_usage());
    }

    #[test]
    fn renders_user_info_request_check_for_markdown() {
        let input = needs_user_info_envelope().to_string();

        let rendered = render_impl(&input, &RenderOptions::Markdown, console(), false).unwrap();

        assert!(rendered.contains("## Check: security"));
        assert!(rendered.contains("- **Target:** `abc1234`"));
        assert!(rendered.contains("- **Status:** needs_user_info"));
        assert!(rendered.contains("### Questions"));
        assert!(rendered.contains(
            "- Which production auth policy applies here, and why does it affect this security check?"
        ));
    }

    #[test]
    fn renders_review_result_as_single_json_document() {
        let result = success_review_result();

        let rendered = render_review_result(&result, RenderOptions::Json, console()).unwrap();
        let value: Value = serde_json::from_str(&rendered).unwrap();

        assert_eq!(value["outcomes"].as_array().unwrap().len(), 2);
        assert_eq!(value["outcomes"][0]["status"], "success");
        assert_eq!(value["outcomes"][0]["check"]["check"], "size");
        assert_eq!(value["outcomes"][1]["check"]["check"], "intent");
        assert!(value["outcomes"][0]["check"].get("usage").is_none());
        assert!(value["outcomes"][1]["check"].get("usage").is_none());
    }

    #[test]
    fn renders_mixed_review_result_as_single_json_document() {
        let result = mixed_review_result();

        let rendered = render_review_result(&result, RenderOptions::Json, console()).unwrap();
        let value: Value = serde_json::from_str(&rendered).unwrap();

        assert_eq!(value["outcomes"].as_array().unwrap().len(), 3);
        assert_eq!(value["outcomes"][2]["status"], "needs_user_info");
        assert_eq!(value["outcomes"][2]["request"]["check"], "security");
        assert!(value["outcomes"][2]["request"].get("usage").is_none());
    }

    #[test]
    fn renders_failed_review_check_in_all_formats() {
        let result = review_result_with_failed_check();

        let terminal =
            render_review_result_impl(&result, &RenderOptions::Terminal, console(), false).unwrap();
        assert!(terminal.contains("Check: security"));
        assert!(terminal.contains("Target: abc1234"));
        assert!(terminal.contains("Status: failed"));
        assert!(terminal.contains("Error: config_invalid — missing API key"));

        let markdown = render_review_result(&result, RenderOptions::Markdown, console()).unwrap();
        assert!(markdown.contains("## Check: security"));
        assert!(markdown.contains("- **Status:** failed"));
        assert!(markdown.contains("`config_invalid`: missing API key"));

        let json = render_review_result(&result, RenderOptions::Json, console()).unwrap();
        let value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["errors"][0]["check"], "security abc1234");
        assert_eq!(value["errors"][0]["error"]["code"], "config_invalid");
        assert_eq!(value["errors"][0]["error"]["message"], "missing API key");
    }

    #[test]
    fn renders_review_result_for_terminal() {
        let result = success_review_result();

        let rendered =
            render_review_result_impl(&result, &RenderOptions::Terminal, console(), false).unwrap();

        assert!(rendered.contains("Check: size"));
        assert!(rendered.contains("Check: intent"));
        assert!(rendered.contains("\n\nCheck: intent"));
        assert!(!rendered.contains("Usage:"));
    }

    #[test]
    fn renders_mixed_review_result_for_terminal() {
        let result = mixed_review_result();

        let rendered =
            render_review_result_impl(&result, &RenderOptions::Terminal, console(), false).unwrap();

        assert!(rendered.contains("Check: size"));
        assert!(rendered.contains("\n\nCheck: security"));
        assert!(rendered.contains("Status: needs_user_info"));
        assert!(rendered.contains(
            "Which production auth policy applies here, and why does it affect this security check?"
        ));
    }

    #[test]
    fn renders_review_result_for_markdown() {
        let result = success_review_result();

        let rendered = render_review_result(&result, RenderOptions::Markdown, console()).unwrap();

        assert!(rendered.contains("## Check: size"));
        assert!(rendered.contains("## Check: intent"));
        assert!(rendered.contains("\n\n## Check: intent"));
        assert!(!rendered.contains("**Usage:**"));
    }

    #[test]
    fn renders_mixed_review_result_for_markdown() {
        let result = mixed_review_result();

        let rendered = render_review_result(&result, RenderOptions::Markdown, console()).unwrap();

        assert!(rendered.contains("## Check: size"));
        assert!(rendered.contains("\n\n## Check: security"));
        assert!(rendered.contains("- **Status:** needs_user_info"));
        assert!(rendered.contains("### Questions"));
    }

    #[test]
    fn folds_all_checks_for_github_review_result() {
        let result = success_review_result();

        let rendered = render_review_result(
            &result,
            RenderOptions::Github {
                repo: "sgkim126/peer".to_string(),
            },
            console(),
        )
        .unwrap();

        assert!(
            rendered.contains(
                "<details>\n<summary>Check: size - Status: ok - Target: abc1234</summary>"
            )
        );
        assert!(rendered.contains(
            "</details>\n\n<details>\n<summary>Check: intent - Status: issue - Target: abc1234</summary>"
        ));
        assert!(rendered.contains("- **Status:** issue"));
    }

    #[test]
    fn folds_non_ok_github_review_outcomes() {
        let result = mixed_review_result();

        let rendered = render_review_result(
            &result,
            RenderOptions::Github {
                repo: "sgkim126/peer".to_string(),
            },
            console(),
        )
        .unwrap();

        assert!(
            rendered.contains("<summary>Check: intent - Status: issue - Target: abc1234</summary>")
        );
        assert!(rendered.contains(
            "<summary>Check: security - Status: needs_user_info - Target: abc1234</summary>"
        ));
        assert!(rendered.contains("- **Status:** needs_user_info"));
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
            &RenderOptions::Terminal,
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
            &RenderOptions::Terminal,
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
            &RenderOptions::Terminal,
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
            RenderOptions::Markdown,
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
    fn renders_successful_check_for_github_with_links() {
        let rendered = render(
            &success_envelope_with_finding().to_string(),
            RenderOptions::Github {
                repo: "sgkim126/peer".to_string(),
            },
            console(),
        )
        .unwrap();

        assert_eq!(
            rendered,
            "\
<details>
<summary>Check: size - Status: issue - Target: abc1234</summary>

## Check: size

- **Target:** [`abc1234`](https://github.com/sgkim126/peer/commit/abc1234)
- **Status:** issue

A critical issue was found.

### Findings

- **critical** — User input reaches a shell command. ([`abc1234`](https://github.com/sgkim126/peer/commit/abc1234) · [`src/main.rs:42`](https://github.com/sgkim126/peer/blob/abc1234/src/main.rs#L42))

### Metadata

- **Confidence:** 85%
- **Iterations:** 2
</details>"
        );
    }

    #[test]
    fn renders_github_finding_file_link_without_line() {
        let mut envelope = success_envelope_with_finding();
        envelope["data"]["check"]["findings"][0]
            .as_object_mut()
            .unwrap()
            .remove("line");

        let rendered = render(
            &envelope.to_string(),
            RenderOptions::Github {
                repo: "sgkim126/peer".to_string(),
            },
            console(),
        )
        .unwrap();

        assert!(rendered.contains(
            "[`src/main.rs`](https://github.com/sgkim126/peer/blob/abc1234/src/main.rs)"
        ));
    }

    #[test]
    fn renders_github_range_target_without_link() {
        let mut envelope = success_envelope();
        envelope["data"]["check"]["target"] = json!("HEAD~2..HEAD");

        let rendered = render(
            &envelope.to_string(),
            RenderOptions::Github {
                repo: "sgkim126/peer".to_string(),
            },
            console(),
        )
        .unwrap();

        assert!(rendered.contains("- **Target:** `HEAD~2..HEAD`"));
    }

    #[test]
    fn folds_ok_check_for_github() {
        let rendered = render(
            &success_envelope().to_string(),
            RenderOptions::Github {
                repo: "sgkim126/peer".to_string(),
            },
            console(),
        )
        .unwrap();

        assert!(rendered.starts_with(
            "<details>\n<summary>Check: size - Status: ok - Target: abc1234</summary>"
        ));
        assert!(rendered.contains("- **Status:** ok"));
        assert!(rendered.ends_with("</details>"));
    }

    #[test]
    fn renders_check_result_for_markdown() {
        let result = success_result_with_finding();

        let rendered =
            render_check_result_impl(&result, &RenderOptions::Markdown, console(), false).unwrap();

        assert!(rendered.contains("## Check: size"));
        assert!(rendered.contains("- **Status:** issue"));
        assert!(rendered.contains("**critical**"));
    }

    #[test]
    fn renders_exhausted_check_warning_for_markdown() {
        let mut envelope = success_envelope();
        envelope["data"]["check"]["is_exhausted"] = json!(true);
        envelope["data"]["check"]["exhaustion_reason"] = json!("max_iterations");

        let rendered = render(&envelope.to_string(), RenderOptions::Markdown, console()).unwrap();

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

        let rendered = render(&envelope.to_string(), RenderOptions::Markdown, console()).unwrap();

        assert_eq!(rendered, "> [!CAUTION]\n> `config_invalid`: invalid config");
    }

    #[test]
    fn rejects_malformed_json() {
        let error = render("{", RenderOptions::Json, console()).unwrap_err();

        assert!(matches!(error, RenderError::InvalidEnvelope(_)));
    }

    #[test]
    fn rejects_envelope_without_status() {
        let input = json!({
            "data": success_envelope()["data"]
        });

        let error = render(&input.to_string(), RenderOptions::Json, console()).unwrap_err();

        assert!(matches!(error, RenderError::InvalidEnvelope(_)));
    }

    #[test]
    fn rejects_invalid_check_envelope_payload() {
        let input = json!({
            "status": "success",
            "data": {}
        });

        let error = render(&input.to_string(), RenderOptions::Json, console()).unwrap_err();

        assert!(matches!(error, RenderError::InvalidEnvelope(_)));
    }
}
