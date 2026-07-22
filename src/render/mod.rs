mod github;
mod markdown;
mod terminal;

use std::fmt;
use std::io::IsTerminal;

use crate::cli::OutputFormat;
use crate::llm::result::CheckResult;

#[derive(Clone, Debug, PartialEq)]
pub struct RenderOptions {
    format: RenderFormat,
}

#[derive(Clone, Debug, PartialEq)]
enum RenderFormat {
    Json,
    Terminal,
    Markdown,
    Github { repo: String },
}

impl RenderOptions {
    pub fn from_cli(
        format: OutputFormat,
        repo: Option<String>,
    ) -> Result<Self, RenderOptionsError> {
        match (format, repo) {
            (OutputFormat::Json, None) => Ok(Self {
                format: RenderFormat::Json,
            }),
            (OutputFormat::Terminal, None) => Ok(Self {
                format: RenderFormat::Terminal,
            }),
            (OutputFormat::Markdown, None) => Ok(Self {
                format: RenderFormat::Markdown,
            }),
            (OutputFormat::Github, Some(repo)) => {
                validate_github_repo(&repo)?;
                Ok(Self {
                    format: RenderFormat::Github { repo },
                })
            }
            (OutputFormat::Github, None) => Err(RenderOptionsError::GithubRepoRequired),
            (_, Some(_)) => Err(RenderOptionsError::RepoRequiresGithubFormat),
        }
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
        RenderFormat::Json => {
            serde_json::to_string_pretty(&result).map_err(RenderError::Serialization)
        }
        RenderFormat::Markdown => Ok(markdown::render(&result)),
        RenderFormat::Terminal => Ok(terminal::render(&result, use_color)),
        RenderFormat::Github { repo } => Ok(github::render(&result, &repo)),
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

fn validate_github_repo(repo: &str) -> Result<(), RenderOptionsError> {
    let Some((owner, name)) = repo.split_once('/') else {
        return Err(RenderOptionsError::MalformedRepo);
    };
    if owner.is_empty()
        || name.is_empty()
        || name.contains('/')
        || !owner.chars().all(is_github_repo_char)
        || !name.chars().all(is_github_repo_char)
    {
        return Err(RenderOptionsError::MalformedRepo);
    }
    Ok(())
}

fn is_github_repo_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-')
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

#[derive(Debug, PartialEq, Eq)]
pub enum RenderOptionsError {
    GithubRepoRequired,
    RepoRequiresGithubFormat,
    MalformedRepo,
}

impl fmt::Display for RenderOptionsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GithubRepoRequired => write!(f, "--format github requires --repo <owner/name>"),
            Self::RepoRequiresGithubFormat => {
                write!(f, "--repo can only be used with --format github")
            }
            Self::MalformedRepo => write!(f, "--repo must use the form owner/name"),
        }
    }
}

impl std::error::Error for RenderOptionsError {}

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
        let options = RenderOptions::from_cli(OutputFormat::Markdown, None).unwrap();
        let output = render(&input, options).unwrap();

        assert!(output.find("abc1234").unwrap() < output.find("def5678").unwrap());
    }

    #[test]
    fn github_requires_a_repo() {
        assert_eq!(
            RenderOptions::from_cli(OutputFormat::Github, None),
            Err(RenderOptionsError::GithubRepoRequired)
        );
    }
}
