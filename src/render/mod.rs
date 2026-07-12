use std::collections::BTreeMap;
use std::fmt;
use std::fmt::Write;
use std::io::IsTerminal;

use crate::cli::OutputFormat;
use crate::console::Console;
use crate::llm::checks::{CheckCommandErrorOutput, CheckCommandOutput, ErrorCode};
use crate::llm::result::{
    CheckOutcome, CheckResult, CheckTarget, CheckUsage, CheckUserInfoRequest, Finding, Severity,
};
use crate::review::{ReviewCheck, ReviewCheckError, ReviewResult};
use owo_colors::Style;

#[derive(Clone, Debug, PartialEq)]
pub struct RenderOptions {
    format: RenderFormat,
    include_usage: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum RenderFormat {
    Json,
    Terminal,
    Markdown,
    Github {
        #[allow(dead_code)]
        repo: String,
    },
}

impl RenderOptions {
    pub const JSON: Self = Self::new(RenderFormat::Json);
    const TERMINAL: Self = Self::new(RenderFormat::Terminal);
    const MARKDOWN: Self = Self::new(RenderFormat::Markdown);

    const fn new(format: RenderFormat) -> Self {
        Self {
            format,
            include_usage: false,
        }
    }

    fn github(repo: String) -> Self {
        Self::new(RenderFormat::Github { repo })
    }

    pub fn from_cli(
        format: OutputFormat,
        github_repo: Option<String>,
        include_usage: bool,
    ) -> Result<Self, RenderOptionsError> {
        match (format, github_repo) {
            (OutputFormat::Json, None) => Ok(Self::with_usage(Self::JSON, include_usage)),
            (OutputFormat::Terminal, None) => Ok(Self::with_usage(Self::TERMINAL, include_usage)),
            (OutputFormat::Markdown, None) => Ok(Self::with_usage(Self::MARKDOWN, include_usage)),
            (OutputFormat::Github, Some(repo)) => {
                validate_github_repo(&repo)?;
                Ok(Self::with_usage(Self::github(repo), include_usage))
            }
            (OutputFormat::Github, None) => Err(RenderOptionsError::MissingGithubRepo),
            (_, Some(_)) => Err(RenderOptionsError::UnexpectedGithubRepo),
        }
    }

    fn include_usage(&self) -> bool {
        self.include_usage
    }

    pub fn with_usage(options: Self, include_usage: bool) -> Self {
        Self {
            include_usage,
            ..options
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
    let include_usage = options.include_usage();

    match &options.format {
        RenderFormat::Json => render_json(output, include_usage),
        RenderFormat::Terminal => Ok(append_check_usage(
            render_terminal(output, use_color),
            output,
            &options.format,
            include_usage,
        )),
        RenderFormat::Markdown => Ok(append_check_usage(
            render_markdown(output),
            output,
            &options.format,
            include_usage,
        )),
        RenderFormat::Github { repo } => Ok(append_check_usage(
            render_github(output, repo),
            output,
            &options.format,
            include_usage,
        )),
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
    use_color: bool,
) -> Result<String, RenderError> {
    let include_usage = options.include_usage();
    match &options.format {
        RenderFormat::Json => {
            render_json(&CheckCommandOutput::success(result.clone()), include_usage)
        }
        RenderFormat::Terminal => Ok(append_usage_text(
            render_terminal_result(result, use_color),
            &result.usage,
            include_usage,
        )),
        RenderFormat::Markdown => Ok(append_usage_markdown(
            render_markdown_result(result),
            &result.usage,
            include_usage,
        )),
        RenderFormat::Github { repo } => Ok(append_usage_github(
            render_github_result(result, repo),
            &result.usage,
            include_usage,
        )),
    }
}

fn render_review_result_impl(
    result: &ReviewResult,
    options: &RenderOptions,
    console: Console,
    use_color: bool,
) -> Result<String, RenderError> {
    log_review_usage(result, console);
    let include_usage = options.include_usage();

    match &options.format {
        RenderFormat::Json => render_review_json(result, include_usage),
        RenderFormat::Terminal | RenderFormat::Markdown | RenderFormat::Github { .. } => {
            let outcomes = result
                .outcomes
                .iter()
                .map(|outcome| render_check_outcome_impl(outcome, options, use_color));
            let errors = result
                .errors
                .iter()
                .map(|error| Ok(render_review_check_error(error, options, use_color)));

            outcomes
                .chain(errors)
                .collect::<Result<Vec<_>, _>>()
                .map(|rendered| {
                    append_review_usage(
                        rendered.join("\n\n"),
                        result,
                        options,
                        include_usage,
                        use_color,
                    )
                })
        }
    }
}

fn render_check_outcome_impl(
    outcome: &CheckOutcome,
    options: &RenderOptions,
    use_color: bool,
) -> Result<String, RenderError> {
    match outcome {
        CheckOutcome::Success { check } => render_check_result_impl(check, options, use_color),
        CheckOutcome::NeedsUserInfo { request } => Ok(match &options.format {
            RenderFormat::Json => unreachable!("review json renders the full review result"),
            RenderFormat::Terminal => append_usage_text(
                render_terminal_user_info_request(request, use_color),
                &request.usage,
                options.include_usage(),
            ),
            RenderFormat::Markdown => append_usage_markdown(
                render_markdown_user_info_request(request),
                &request.usage,
                options.include_usage(),
            ),
            RenderFormat::Github { .. } => append_usage_github(
                render_github_user_info_request(request),
                &request.usage,
                options.include_usage(),
            ),
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

fn render_json(output: &CheckCommandOutput, include_usage: bool) -> Result<String, RenderError> {
    let mut value = serde_json::to_value(output).map_err(RenderError::Serialization)?;
    if !include_usage {
        remove_usage(&mut value);
    }
    serde_json::to_string_pretty(&value).map_err(RenderError::Serialization)
}

fn render_review_json(result: &ReviewResult, include_usage: bool) -> Result<String, RenderError> {
    let mut value = serde_json::to_value(result).map_err(RenderError::Serialization)?;
    if !include_usage {
        remove_review_usage(&mut value);
    }
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

fn append_check_usage(
    rendered: String,
    output: &CheckCommandOutput,
    format: &RenderFormat,
    include_usage: bool,
) -> String {
    let Ok(outcome) = output.as_outcome() else {
        return rendered;
    };
    let usage = match outcome {
        CheckOutcome::Success { check } => &check.usage,
        CheckOutcome::NeedsUserInfo { request } => &request.usage,
    };
    match format {
        RenderFormat::Terminal => append_usage_text(rendered, usage, include_usage),
        RenderFormat::Markdown => append_usage_markdown(rendered, usage, include_usage),
        RenderFormat::Github { .. } => append_usage_github(rendered, usage, include_usage),
        RenderFormat::Json => unreachable!("JSON usage is serialized with the result"),
    }
}

fn append_usage_text(rendered: String, usage: &CheckUsage, include_usage: bool) -> String {
    if include_usage {
        format!(
            "{rendered}\n\nUsage: {} input, {} output, ${:.6} ({})",
            usage.input_tokens, usage.output_tokens, usage.cost_usd, usage.model
        )
    } else {
        rendered
    }
}

fn append_usage_markdown(rendered: String, usage: &CheckUsage, include_usage: bool) -> String {
    if include_usage {
        format!(
            "{rendered}\n\n### Usage\n\n- **Input tokens:** {}\n- **Output tokens:** {}\n- **Cost:** ${:.6}\n- **Model:** {}",
            usage.input_tokens, usage.output_tokens, usage.cost_usd, usage.model
        )
    } else {
        rendered
    }
}

fn append_usage_github(rendered: String, usage: &CheckUsage, include_usage: bool) -> String {
    if !include_usage {
        rendered
    } else if let Some(content) = rendered.strip_suffix("\n</details>") {
        format!(
            "{content}\n\n### Usage\n\n- **Input tokens:** {}\n- **Output tokens:** {}\n- **Cost:** ${:.6}\n- **Model:** {}\n</details>",
            usage.input_tokens, usage.output_tokens, usage.cost_usd, usage.model
        )
    } else {
        append_usage_markdown(rendered, usage, true)
    }
}

fn append_review_usage(
    rendered: String,
    result: &ReviewResult,
    options: &RenderOptions,
    include_usage: bool,
    use_color: bool,
) -> String {
    if !include_usage {
        return rendered;
    }

    let usages = result.outcomes.iter().map(|outcome| match outcome {
        CheckOutcome::Success { check } => &check.usage,
        CheckOutcome::NeedsUserInfo { request } => &request.usage,
    });
    let totals = aggregate_usage_by_model(usages);
    if totals.is_empty() {
        return rendered;
    }

    match &options.format {
        RenderFormat::Terminal => append_review_usage_terminal(rendered, &totals, use_color),
        RenderFormat::Markdown => append_review_usage_markdown(rendered, &totals),
        RenderFormat::Github { .. } => totals.into_iter().fold(rendered, |output, usage| {
            append_usage_github_summary(output, &usage)
        }),
        RenderFormat::Json => unreachable!("JSON usage is serialized with the result"),
    }
}

fn append_review_usage_terminal(
    rendered: String,
    usages: &[CheckUsage],
    use_color: bool,
) -> String {
    let by_model = usages
        .iter()
        .map(|usage| {
            format!(
                "- {}: {} input, {} output, {}",
                styled(&usage.model, Style::new().cyan().bold(), use_color),
                usage.input_tokens,
                usage.output_tokens,
                styled(
                    format!("${:.6}", usage.cost_usd),
                    Style::new().green().bold(),
                    use_color
                )
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!("{rendered}\n\nUsage summary by model:\n{by_model}")
}

fn append_review_usage_markdown(rendered: String, usages: &[CheckUsage]) -> String {
    let by_model = usages
        .iter()
        .map(|usage| {
            format!(
                "### {}\n\n- **Input tokens:** {}\n- **Output tokens:** {}\n- **Cost:** ${:.6}",
                usage.model, usage.input_tokens, usage.output_tokens, usage.cost_usd
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    format!("{rendered}\n\n## Usage\n\n{by_model}")
}

fn append_usage_github_summary(rendered: String, usage: &CheckUsage) -> String {
    format!(
        "{rendered}\n\n<details>\n<summary>Usage: {}</summary>\n\n- **Input tokens:** {}\n- **Output tokens:** {}\n- **Cost:** ${:.6}\n</details>",
        usage.model, usage.input_tokens, usage.output_tokens, usage.cost_usd
    )
}

fn log_usage(output: &CheckCommandOutput, console: Console) {
    if let Ok(CheckOutcome::Success { check }) = output.as_outcome() {
        log_check_usage(&check.usage, console);
    }
}

fn log_review_usage(result: &ReviewResult, console: Console) {
    let usages = result.outcomes.iter().filter_map(|outcome| match outcome {
        CheckOutcome::Success { check } => Some(&check.usage),
        CheckOutcome::NeedsUserInfo { .. } => None,
    });

    for usage in aggregate_usage_by_model(usages) {
        log_check_usage(&usage, console);
    }
}

fn log_check_usage(usage: &CheckUsage, console: Console) {
    console.verbose(format_args!(
        "Usage: {} input, {} output, ${:.6} ({})",
        usage.input_tokens, usage.output_tokens, usage.cost_usd, usage.model
    ));
}

fn aggregate_usage_by_model<'a>(
    usages: impl IntoIterator<Item = &'a CheckUsage>,
) -> Vec<CheckUsage> {
    let mut totals = BTreeMap::new();

    for usage in usages {
        totals
            .entry(usage.model.clone())
            .and_modify(|total: &mut CheckUsage| {
                total.input_tokens = total.input_tokens.saturating_add(usage.input_tokens);
                total.output_tokens = total.output_tokens.saturating_add(usage.output_tokens);
                total.cost_usd += usage.cost_usd;
            })
            .or_insert_with(|| usage.clone());
    }

    totals.into_values().collect()
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
        "{} {}",
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

    match &options.format {
        RenderFormat::Terminal => format!(
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
        RenderFormat::Markdown => render_markdown_review_check_error(check, target, &error),
        RenderFormat::Github { .. } => format!(
            "<details>\n<summary>Check: {check} - Status: failed - Target: {target}</summary>\n\n{}\n</details>",
            render_markdown_review_check_error(check, target, &error)
        ),
        RenderFormat::Json => unreachable!("review json is rendered separately"),
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

    #[test]
    fn aggregates_review_usage_by_model() {
        let model1 = "other-model";
        let input_tokens1 = 50;
        let output_tokens1 = 10;
        let cost_usd1 = 0.002;

        let mut result = success_review_result();
        let CheckOutcome::Success { check } = &mut result.outcomes[1] else {
            unreachable!();
        };
        check.usage.model = model1.to_string();
        check.usage.input_tokens = input_tokens1;
        check.usage.output_tokens = output_tokens1;
        check.usage.cost_usd = cost_usd1;

        let mut another = success_result();
        let input_tokens2 = 5;
        let output_tokens2 = 10;
        let cost_usd2 = 0.0001;
        another.usage.input_tokens = input_tokens2;
        another.usage.output_tokens = output_tokens2;
        another.usage.cost_usd = cost_usd2;
        result.outcomes.push(CheckOutcome::success(another));

        let usages = result.outcomes.iter().filter_map(|outcome| match outcome {
            CheckOutcome::Success { check } => Some(&check.usage),
            CheckOutcome::NeedsUserInfo { .. } => None,
        });
        let totals = aggregate_usage_by_model(usages);

        assert_eq!(totals.len(), 2);
        assert_eq!(totals[0].model, model1);
        assert_eq!(totals[0].input_tokens, input_tokens1);
        assert_eq!(totals[0].output_tokens, output_tokens1);
        assert_eq!(totals[0].cost_usd, cost_usd1);
        assert_eq!(totals[1].model, "test-model");
        assert_eq!(totals[1].input_tokens, 100 + input_tokens2);
        assert_eq!(totals[1].output_tokens, 20 + output_tokens2);
        assert_eq!(totals[1].cost_usd, 0.001 + cost_usd2);
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
        assert_eq!(
            RenderOptions::from_cli(OutputFormat::Json, None, false).unwrap(),
            RenderOptions::JSON
        );
        assert_eq!(
            RenderOptions::from_cli(OutputFormat::Terminal, None, false).unwrap(),
            RenderOptions::TERMINAL
        );
        assert_eq!(
            RenderOptions::from_cli(OutputFormat::Markdown, None, false).unwrap(),
            RenderOptions::MARKDOWN
        );
        assert_eq!(
            RenderOptions::from_cli(OutputFormat::Github, Some("sgkim126/peer".into()), false)
                .unwrap()
                .format,
            RenderFormat::Github {
                repo: "sgkim126/peer".into()
            }
        );
    }

    #[test]
    fn rejects_invalid_render_options_from_cli_arguments() {
        assert_eq!(
            RenderOptions::from_cli(OutputFormat::Github, None, false).unwrap_err(),
            RenderOptionsError::MissingGithubRepo
        );
        assert_eq!(
            RenderOptions::from_cli(OutputFormat::Markdown, Some("sgkim126/peer".into()), false)
                .unwrap_err(),
            RenderOptionsError::UnexpectedGithubRepo
        );
        assert_eq!(
            RenderOptions::from_cli(OutputFormat::Github, Some("sgkim126".into()), false)
                .unwrap_err(),
            RenderOptionsError::InvalidGithubRepo
        );
        assert_eq!(
            RenderOptions::from_cli(
                OutputFormat::Github,
                Some("sgkim126/peer/extra".into()),
                false
            )
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

        let rendered = render(&input, RenderOptions::JSON, console()).unwrap();

        assert!(rendered.contains('\n'));
        assert_eq!(
            serde_json::from_str::<Value>(&rendered).unwrap(),
            success_envelope_without_usage()
        );
    }

    #[test]
    fn renders_successful_check_for_terminal() {
        let input = success_envelope_with_finding().to_string();

        let rendered = render_impl(&input, &RenderOptions::TERMINAL, console(), false).unwrap();

        assert_eq!(
            rendered,
            "\
Check: size
Target: abc1234
Status: issue

A critical issue was found.

Findings:
- [critical] User input reaches a shell command. (abc1234 src/main.rs:42)

Iterations: 2"
        );
    }

    #[test]
    fn renders_user_info_request_check_for_terminal() {
        let input = needs_user_info_envelope().to_string();

        let rendered = render_impl(&input, &RenderOptions::TERMINAL, console(), false).unwrap();

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

        let rendered = render_impl(&input, &RenderOptions::TERMINAL, console(), false).unwrap();

        assert!(!rendered.contains("Usage:"));
        assert!(!rendered.contains("test-model"));
    }

    #[test]
    fn includes_usage_in_terminal_output_when_requested() {
        let input = success_envelope_with_finding().to_string();

        let rendered = render_impl(
            &input,
            &RenderOptions::with_usage(RenderOptions::TERMINAL, true),
            console(),
            false,
        )
        .unwrap();

        assert!(rendered.contains("Usage: 100 input, 20 output, $0.001000 (test-model)"));
    }

    #[test]
    fn includes_usage_in_json_output_when_requested() {
        let input = success_envelope().to_string();

        let rendered = render(
            &input,
            RenderOptions::with_usage(RenderOptions::JSON, true),
            console(),
        )
        .unwrap();
        let value: Value = serde_json::from_str(&rendered).unwrap();

        assert_eq!(value["data"]["check"]["usage"]["input_tokens"], 100);
        assert_eq!(value["data"]["check"]["usage"]["cost_usd"], 0.001);
    }

    #[test]
    fn renders_check_result_for_terminal() {
        let result = success_result_with_finding();

        let rendered = render_check_result_impl(&result, &RenderOptions::TERMINAL, false).unwrap();

        assert!(rendered.contains("Check: size"));
        assert!(rendered.contains("Status: issue"));
        assert!(rendered.contains("User input reaches a shell command."));
    }

    #[test]
    fn renders_check_result_as_pretty_json_envelope() {
        let result = success_result();

        let rendered = render_check_result_impl(&result, &RenderOptions::JSON, false).unwrap();
        let value: Value = serde_json::from_str(&rendered).unwrap();

        assert_eq!(value, success_envelope_without_usage());
    }

    #[test]
    fn renders_user_info_request_check_for_markdown() {
        let input = needs_user_info_envelope().to_string();

        let rendered = render_impl(&input, &RenderOptions::MARKDOWN, console(), false).unwrap();

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

        let rendered = render_review_result(&result, RenderOptions::JSON, console()).unwrap();
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

        let rendered = render_review_result(&result, RenderOptions::JSON, console()).unwrap();
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
            render_review_result_impl(&result, &RenderOptions::TERMINAL, console(), false).unwrap();
        assert!(terminal.contains("Check: security"));
        assert!(terminal.contains("Target: abc1234"));
        assert!(terminal.contains("Status: failed"));
        assert!(terminal.contains("Error: config_invalid — missing API key"));

        let markdown = render_review_result(&result, RenderOptions::MARKDOWN, console()).unwrap();
        assert!(markdown.contains("## Check: security"));
        assert!(markdown.contains("- **Status:** failed"));
        assert!(markdown.contains("`config_invalid`: missing API key"));

        let json = render_review_result(&result, RenderOptions::JSON, console()).unwrap();
        let value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["errors"][0]["check"], "security abc1234");
        assert_eq!(value["errors"][0]["error"]["code"], "config_invalid");
        assert_eq!(value["errors"][0]["error"]["message"], "missing API key");
    }

    #[test]
    fn renders_review_result_for_terminal() {
        let result = success_review_result();

        let rendered =
            render_review_result_impl(&result, &RenderOptions::TERMINAL, console(), false).unwrap();

        assert!(rendered.contains("Check: size"));
        assert!(rendered.contains("Check: intent"));
        assert!(rendered.contains("\n\nCheck: intent"));
        assert!(!rendered.contains("Usage:"));
    }

    #[test]
    fn renders_mixed_review_result_for_terminal() {
        let result = mixed_review_result();

        let rendered =
            render_review_result_impl(&result, &RenderOptions::TERMINAL, console(), false).unwrap();

        assert!(rendered.contains("Check: size"));
        assert!(rendered.contains("\n\nCheck: security"));
        assert!(rendered.contains("Status: needs_user_info"));
        assert!(rendered.contains(
            "Which production auth policy applies here, and why does it affect this security check?"
        ));
    }

    #[test]
    fn renders_aggregated_usage_at_review_level_in_terminal() {
        let result = success_review_result();

        let rendered = render_review_result_impl(
            &result,
            &RenderOptions::with_usage(RenderOptions::TERMINAL, true),
            console(),
            false,
        )
        .unwrap();

        assert!(
            rendered.ends_with(
                "Usage summary by model:\n- test-model: 200 input, 40 output, $0.002000"
            )
        );
    }

    #[test]
    fn styles_model_and_cost_in_terminal_usage_summary() {
        let result = success_review_result();

        let rendered = render_review_result_impl(
            &result,
            &RenderOptions::with_usage(RenderOptions::TERMINAL, true),
            console(),
            true,
        )
        .unwrap();
        let summary = rendered.rsplit_once("Usage summary by model:").unwrap().1;

        assert!(summary.contains("\u{1b}["));
        assert!(summary.contains("test-model"));
        assert!(summary.contains("$0.002000"));
    }

    #[test]
    fn renders_review_result_for_markdown() {
        let result = success_review_result();

        let rendered = render_review_result(&result, RenderOptions::MARKDOWN, console()).unwrap();

        assert!(rendered.contains("## Check: size"));
        assert!(rendered.contains("## Check: intent"));
        assert!(rendered.contains("\n\n## Check: intent"));
        assert!(!rendered.contains("**Usage:**"));
    }

    #[test]
    fn renders_aggregated_usage_at_review_level_in_markdown() {
        let result = success_review_result();

        let rendered = render_review_result(
            &result,
            RenderOptions::with_usage(RenderOptions::MARKDOWN, true),
            console(),
        )
        .unwrap();

        assert!(rendered.contains(
            "\n\n## Usage\n\n### test-model\n\n- **Input tokens:** 200\n- **Output tokens:** 40\n- **Cost:** $0.002000"
        ));
    }

    #[test]
    fn renders_mixed_review_result_for_markdown() {
        let result = mixed_review_result();

        let rendered = render_review_result(&result, RenderOptions::MARKDOWN, console()).unwrap();

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
            RenderOptions::github("sgkim126/peer".to_string()),
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
    fn folds_aggregated_usage_in_github_review_output() {
        let mut result = success_review_result();
        let CheckOutcome::Success { check } = &mut result.outcomes[1] else {
            unreachable!();
        };
        check.usage.model = "other-model".to_string();
        check.usage.input_tokens = 50;
        check.usage.output_tokens = 10;
        check.usage.cost_usd = 0.002;

        let rendered = render_review_result(
            &result,
            RenderOptions::with_usage(RenderOptions::github("sgkim126/peer".to_string()), true),
            console(),
        )
        .unwrap();

        assert!(rendered.contains(
            "<details>\n<summary>Usage: other-model</summary>\n\n- **Input tokens:** 50\n- **Output tokens:** 10\n- **Cost:** $0.002000\n</details>"
        ));
        assert!(rendered.contains(
            "<details>\n<summary>Usage: test-model</summary>\n\n- **Input tokens:** 100\n- **Output tokens:** 20\n- **Cost:** $0.001000\n</details>"
        ));
        assert!(rendered.contains(
            "### Usage\n\n- **Input tokens:** 100\n- **Output tokens:** 20\n- **Cost:** $0.001000\n- **Model:** test-model\n</details>"
        ));
        assert!(rendered.contains(
            "### Usage\n\n- **Input tokens:** 50\n- **Output tokens:** 10\n- **Cost:** $0.002000\n- **Model:** other-model\n</details>"
        ));
    }

    #[test]
    fn folds_non_ok_github_review_outcomes() {
        let result = mixed_review_result();

        let rendered = render_review_result(
            &result,
            RenderOptions::github("sgkim126/peer".to_string()),
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
            &RenderOptions::TERMINAL,
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
            &RenderOptions::TERMINAL,
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
            &RenderOptions::TERMINAL,
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
            RenderOptions::MARKDOWN,
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

- **Iterations:** 2"
        );
    }

    #[test]
    fn renders_successful_check_for_github_with_links() {
        let rendered = render(
            &success_envelope_with_finding().to_string(),
            RenderOptions::github("sgkim126/peer".to_string()),
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
            RenderOptions::github("sgkim126/peer".to_string()),
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
            RenderOptions::github("sgkim126/peer".to_string()),
            console(),
        )
        .unwrap();

        assert!(rendered.contains("- **Target:** `HEAD~2..HEAD`"));
    }

    #[test]
    fn folds_ok_check_for_github() {
        let rendered = render(
            &success_envelope().to_string(),
            RenderOptions::github("sgkim126/peer".to_string()),
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

        let rendered = render_check_result_impl(&result, &RenderOptions::MARKDOWN, false).unwrap();

        assert!(rendered.contains("## Check: size"));
        assert!(rendered.contains("- **Status:** issue"));
        assert!(rendered.contains("**critical**"));
    }

    #[test]
    fn renders_exhausted_check_warning_for_markdown() {
        let mut envelope = success_envelope();
        envelope["data"]["check"]["is_exhausted"] = json!(true);
        envelope["data"]["check"]["exhaustion_reason"] = json!("max_iterations");

        let rendered = render(&envelope.to_string(), RenderOptions::MARKDOWN, console()).unwrap();

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

        let rendered = render(&envelope.to_string(), RenderOptions::MARKDOWN, console()).unwrap();

        assert_eq!(rendered, "> [!CAUTION]\n> `config_invalid`: invalid config");
    }

    #[test]
    fn rejects_malformed_json() {
        let error = render("{", RenderOptions::JSON, console()).unwrap_err();

        assert!(matches!(error, RenderError::InvalidEnvelope(_)));
    }

    #[test]
    fn rejects_envelope_without_status() {
        let input = json!({
            "data": success_envelope()["data"]
        });

        let error = render(&input.to_string(), RenderOptions::JSON, console()).unwrap_err();

        assert!(matches!(error, RenderError::InvalidEnvelope(_)));
    }

    #[test]
    fn rejects_invalid_check_envelope_payload() {
        let input = json!({
            "status": "success",
            "data": {}
        });

        let error = render(&input.to_string(), RenderOptions::JSON, console()).unwrap_err();

        assert!(matches!(error, RenderError::InvalidEnvelope(_)));
    }
}
