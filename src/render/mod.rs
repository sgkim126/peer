mod markdown;
mod terminal;

use std::fmt;
use std::io::IsTerminal;

use crate::cli::OutputFormat;
use crate::llm::result::CheckResult;

#[derive(Clone, Debug, PartialEq)]
pub struct RenderOptions {
    format: OutputFormat,
}

impl RenderOptions {
    pub fn from_cli(format: OutputFormat) -> Self {
        Self { format }
    }
}

pub fn render(input: &str, options: RenderOptions) -> Result<String, RenderError> {
    render_impl(input, options, std::io::stdout().is_terminal())
}

fn render_impl(
    input: &str,
    options: RenderOptions,
    use_color: bool,
) -> Result<String, RenderError> {
    let result: CheckResult = serde_json::from_str(input).map_err(RenderError::InvalidResult)?;
    let result = sort_findings(result);

    match options.format {
        OutputFormat::Json => {
            serde_json::to_string_pretty(&result).map_err(RenderError::Serialization)
        }
        OutputFormat::Markdown => Ok(markdown::render(&result)),
        OutputFormat::Terminal => Ok(terminal::render(&result, use_color)),
    }
}

fn sort_findings(mut result: CheckResult) -> CheckResult {
    result.findings.sort_by_key(|finding| {
        result
            .ordered_commits
            .iter()
            .position(|commit| commit.as_ref().starts_with(finding.commit.as_ref()))
            .unwrap_or(usize::MAX)
    });
    result
}

#[derive(Debug)]
pub enum RenderError {
    InvalidResult(serde_json::Error),
    Serialization(serde_json::Error),
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidResult(error) => write!(f, "invalid check result: {error}"),
            Self::Serialization(error) => write!(f, "cannot serialize check result: {error}"),
        }
    }
}

impl std::error::Error for RenderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidResult(error) => Some(error),
            Self::Serialization(error) => Some(error),
        }
    }
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
    fn orders_findings_with_abbreviated_commit_hashes() {
        let mut result = result();
        result.ordered_commits = vec![
            CommitHash::new(&format!("abc1234{}", "0".repeat(33))).unwrap(),
            CommitHash::new(&format!("def5678{}", "0".repeat(33))).unwrap(),
        ];

        let result = sort_findings(result);

        assert_eq!(result.findings[0].commit.as_ref(), "abc1234");
        assert_eq!(result.findings[1].commit.as_ref(), "def5678");
    }

    #[test]
    fn render_orders_findings_by_commit_order() {
        let input = serde_json::to_string(&result()).unwrap();
        let output = render(&input, RenderOptions::from_cli(OutputFormat::Markdown)).unwrap();

        assert!(output.find("abc1234").unwrap() < output.find("def5678").unwrap());
    }
}
