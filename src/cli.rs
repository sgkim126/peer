use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(name = "peer", version, about = "LLM-based code review CLI")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug, PartialEq)]
pub enum Command {
    Init,

    /// Remove cached values.
    Prune {
        /// Remove all cached values, including values for the current version.
        #[arg(long)]
        all: bool,
    },

    Review {
        target: String,

        #[arg(long)]
        provider: Option<String>,

        #[arg(long)]
        model: Option<String>,

        #[arg(long)]
        title: Option<String>,

        #[arg(long)]
        body_file: Option<PathBuf>,

        #[arg(long)]
        comments_file: Option<PathBuf>,

        /// Start resumable stages from the beginning.
        #[arg(long)]
        no_resume: bool,
    },

    Render {
        #[arg(long, default_value = "terminal")]
        format: OutputFormat,

        #[arg(long, required_if_eq("format", "github"))]
        repo: Option<String>,
    },
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq)]
pub enum OutputFormat {
    Terminal,
    Markdown,
    Github,
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::assert_matches;

    fn parse(args: &[&str]) -> Cli {
        Cli::parse_from(args)
    }

    #[test]
    fn reports_the_package_version() {
        let error = Cli::try_parse_from(["peer", "--version"]).unwrap_err();

        assert_eq!(error.kind(), clap::error::ErrorKind::DisplayVersion);
        assert_eq!(
            error.to_string(),
            format!("peer {}\n", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn init() {
        let cli = parse(&["peer", "init"]);

        assert_eq!(cli.command, Command::Init);
    }

    #[test]
    fn prune() {
        let cli = parse(&["peer", "prune"]);

        assert_eq!(cli.command, Command::Prune { all: false });
    }

    #[test]
    fn prune_all() {
        let cli = parse(&["peer", "prune", "--all"]);

        assert_eq!(cli.command, Command::Prune { all: true });
    }

    #[test]
    fn review() {
        let cli = parse(&["peer", "review", "HEAD~3..HEAD"]);

        assert_eq!(
            cli.command,
            Command::Review {
                target: "HEAD~3..HEAD".into(),
                provider: None,
                model: None,
                title: None,
                body_file: None,
                comments_file: None,
                no_resume: false,
            }
        );
    }

    #[test]
    fn review_rejects_format() {
        let result = Cli::try_parse_from(["peer", "review", "HEAD", "--format", "json"]);

        assert_matches!(result, Err(_));
    }

    #[test]
    fn review_with_review_context() {
        let cli = parse(&[
            "peer",
            "review",
            "HEAD",
            "--title",
            "Add context compression",
            "--body-file",
            "body.md",
            "--comments-file",
            "comments.json",
        ]);

        assert_eq!(
            cli.command,
            Command::Review {
                target: "HEAD".into(),
                provider: None,
                model: None,
                title: Some("Add context compression".into()),
                body_file: Some("body.md".into()),
                comments_file: Some("comments.json".into()),
                no_resume: false,
            }
        );
    }

    #[test]
    fn review_with_provider_and_model_overrides() {
        let cli = parse(&[
            "peer",
            "review",
            "HEAD",
            "--provider",
            "openai",
            "--model",
            "gpt-5.6-terra",
        ]);

        assert_eq!(
            cli.command,
            Command::Review {
                target: "HEAD".into(),
                provider: Some("openai".into()),
                model: Some("gpt-5.6-terra".into()),
                title: None,
                body_file: None,
                comments_file: None,
                no_resume: false,
            }
        );
    }

    #[test]
    fn review_accepts_arbitrary_provider_override() {
        let cli = parse(&["peer", "review", "HEAD", "--provider", "custom"]);

        assert_matches!(
            cli.command,
            Command::Review {
                provider: Some(provider),
                ..
            } if provider == "custom"
        );
    }

    #[test]
    fn review_without_resuming() {
        let cli = parse(&["peer", "review", "HEAD", "--no-resume"]);

        assert_matches!(
            cli.command,
            Command::Review {
                no_resume: true,
                ..
            }
        );
    }

    #[test]
    fn extract_is_not_a_command() {
        let result = Cli::try_parse_from(["peer", "extract"]);

        assert_matches!(result, Err(_));
    }

    #[test]
    fn render_with_default_format() {
        let cli = parse(&["peer", "render"]);

        assert_eq!(
            cli.command,
            Command::Render {
                format: OutputFormat::Terminal,
                repo: None,
            }
        );
    }

    #[test]
    fn render_rejects_json_format() {
        let result = Cli::try_parse_from(["peer", "render", "--format", "json"]);

        assert_matches!(result, Err(_));
    }

    #[test]
    fn render_with_github_format_and_repo() {
        let cli = parse(&[
            "peer",
            "render",
            "--format",
            "github",
            "--repo",
            "owner/repository",
        ]);

        assert_eq!(
            cli.command,
            Command::Render {
                format: OutputFormat::Github,
                repo: Some("owner/repository".into()),
            }
        );
    }

    #[test]
    fn render_with_github_format_requires_repo() {
        let result = Cli::try_parse_from(["peer", "render", "--format", "github"]);

        assert_matches!(result, Err(_));
    }
}
