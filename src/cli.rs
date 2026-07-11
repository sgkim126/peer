use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

use crate::review::ReviewCheckKind;

#[derive(Parser, Debug)]
#[command(name = "peer", about = "LLM-based code review CLI")]
pub struct Cli {
    #[arg(long, global = true)]
    pub verbose: bool,

    #[arg(long, global = true)]
    pub debug: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug, PartialEq)]
pub enum Command {
    Init,

    /// Remove cache entries created by older peer versions.
    Prune,

    Review {
        target: String,

        #[arg(long)]
        provider: Option<String>,

        #[arg(long)]
        model: Option<String>,

        #[arg(
            long = "skip-check",
            value_enum,
            value_delimiter = ',',
            conflicts_with = "only_checks"
        )]
        skip_checks: Vec<ReviewCheckKind>,

        #[arg(
            long = "only-check",
            value_enum,
            value_delimiter = ',',
            conflicts_with = "skip_checks"
        )]
        only_checks: Vec<ReviewCheckKind>,

        #[arg(long)]
        title: Option<String>,

        #[arg(long)]
        body_file: Option<PathBuf>,

        #[arg(long)]
        comments_file: Option<PathBuf>,

        #[arg(long, default_value = "terminal")]
        format: OutputFormat,

        #[arg(long)]
        repo: Option<String>,
    },

    Extract {
        #[command(subcommand)]
        command: ExtractCommand,
    },

    Check {
        #[arg(long, global = true)]
        provider: Option<String>,

        #[arg(long, global = true)]
        model: Option<String>,

        #[command(subcommand)]
        command: CheckCommand,
    },

    Render {
        #[arg(long, default_value = "terminal")]
        format: OutputFormat,

        #[arg(long)]
        repo: Option<String>,
    },
}

#[derive(Subcommand, Debug, PartialEq)]
#[command(rename_all = "kebab-case")]
pub enum ExtractCommand {
    CommitMessage {
        revision: String,
    },
    CommitDiff {
        revision: String,
    },
    CommitFiles {
        revision: String,
    },
    CommitList {
        range: String,
    },
    FileContent {
        revision: String,
        #[arg(long)]
        path: PathBuf,
    },
}

#[derive(Subcommand, Debug, PartialEq)]
#[command(rename_all = "kebab-case")]
pub enum CheckCommand {
    Size { revision: String },
    Intent { revision: String },
    Quality { revision: String },
    Security { revision: String },
    Coherence { range: String },
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq)]
pub enum OutputFormat {
    Json,
    Terminal,
    Markdown,
    Github,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Cli {
        Cli::parse_from(args)
    }

    #[test]
    fn init() {
        let cli = parse(&["peer", "init"]);

        assert_eq!(cli.command, Command::Init);
        assert!(!cli.verbose);
        assert!(!cli.debug);
    }

    #[test]
    fn prune() {
        let cli = parse(&["peer", "prune"]);

        assert_eq!(cli.command, Command::Prune);
    }

    #[test]
    fn review_with_default_format() {
        let cli = parse(&["peer", "review", "HEAD~3..HEAD"]);

        assert_eq!(
            cli.command,
            Command::Review {
                target: "HEAD~3..HEAD".into(),
                provider: None,
                model: None,
                skip_checks: vec![],
                only_checks: vec![],
                title: None,
                body_file: None,
                comments_file: None,
                format: OutputFormat::Terminal,
                repo: None,
            }
        );
    }

    #[test]
    fn review_with_json_format() {
        let cli = parse(&["peer", "review", "abc123", "--format", "json"]);

        assert_eq!(
            cli.command,
            Command::Review {
                target: "abc123".into(),
                provider: None,
                model: None,
                skip_checks: vec![],
                only_checks: vec![],
                title: None,
                body_file: None,
                comments_file: None,
                format: OutputFormat::Json,
                repo: None,
            }
        );
    }

    #[test]
    fn review_with_markdown_format() {
        let cli = parse(&["peer", "review", "main", "--format", "markdown"]);

        assert_eq!(
            cli.command,
            Command::Review {
                target: "main".into(),
                provider: None,
                model: None,
                skip_checks: vec![],
                only_checks: vec![],
                title: None,
                body_file: None,
                comments_file: None,
                format: OutputFormat::Markdown,
                repo: None,
            }
        );
    }

    #[test]
    fn review_with_github_format_and_repo() {
        let cli = parse(&[
            "peer",
            "review",
            "main",
            "--format",
            "github",
            "--repo",
            "sgkim126/peer",
        ]);

        assert_eq!(
            cli.command,
            Command::Review {
                target: "main".into(),
                provider: None,
                model: None,
                skip_checks: vec![],
                only_checks: vec![],
                title: None,
                body_file: None,
                comments_file: None,
                format: OutputFormat::Github,
                repo: Some("sgkim126/peer".into()),
            }
        );
    }

    #[test]
    fn review_with_context_options() {
        let cli = parse(&[
            "peer",
            "review",
            "HEAD",
            "--title",
            "Add review context",
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
                skip_checks: vec![],
                only_checks: vec![],
                title: Some("Add review context".into()),
                body_file: Some(PathBuf::from("body.md")),
                comments_file: Some(PathBuf::from("comments.json")),
                format: OutputFormat::Terminal,
                repo: None,
            }
        );
    }

    #[test]
    fn review_with_skip_checks() {
        let cli = parse(&[
            "peer",
            "review",
            "HEAD~3..HEAD",
            "--skip-check",
            "size,security",
            "--skip-check",
            "coherence",
        ]);

        assert_eq!(
            cli.command,
            Command::Review {
                target: "HEAD~3..HEAD".into(),
                provider: None,
                model: None,
                skip_checks: vec![
                    ReviewCheckKind::Size,
                    ReviewCheckKind::Security,
                    ReviewCheckKind::Coherence,
                ],
                only_checks: vec![],
                title: None,
                body_file: None,
                comments_file: None,
                format: OutputFormat::Terminal,
                repo: None,
            }
        );
    }

    #[test]
    fn review_with_only_checks() {
        let cli = parse(&[
            "peer",
            "review",
            "HEAD~3..HEAD",
            "--only-check",
            "size,security",
            "--only-check",
            "coherence",
        ]);

        assert_eq!(
            cli.command,
            Command::Review {
                target: "HEAD~3..HEAD".into(),
                provider: None,
                model: None,
                skip_checks: vec![],
                only_checks: vec![
                    ReviewCheckKind::Size,
                    ReviewCheckKind::Security,
                    ReviewCheckKind::Coherence,
                ],
                title: None,
                body_file: None,
                comments_file: None,
                format: OutputFormat::Terminal,
                repo: None,
            }
        );
    }

    #[test]
    fn review_rejects_combining_only_and_skip_checks() {
        let error = Cli::try_parse_from([
            "peer",
            "review",
            "HEAD",
            "--only-check",
            "size",
            "--skip-check",
            "security",
        ])
        .unwrap_err();

        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn review_with_provider_and_model() {
        let cli = parse(&[
            "peer",
            "review",
            "HEAD",
            "--provider",
            "openai",
            "--model",
            "gpt-5.4-mini",
        ]);

        assert_eq!(
            cli.command,
            Command::Review {
                target: "HEAD".into(),
                provider: Some("openai".into()),
                model: Some("gpt-5.4-mini".into()),
                skip_checks: vec![],
                only_checks: vec![],
                title: None,
                body_file: None,
                comments_file: None,
                format: OutputFormat::Terminal,
                repo: None,
            }
        );
    }

    #[test]
    fn extract_commit_message() {
        let cli = parse(&["peer", "extract", "commit-message", "abc123"]);

        assert_eq!(
            cli.command,
            Command::Extract {
                command: ExtractCommand::CommitMessage {
                    revision: "abc123".into()
                },
            }
        );
    }

    #[test]
    fn extract_commit_diff() {
        let cli = parse(&["peer", "extract", "commit-diff", "abc123"]);

        assert_eq!(
            cli.command,
            Command::Extract {
                command: ExtractCommand::CommitDiff {
                    revision: "abc123".into()
                },
            }
        );
    }

    #[test]
    fn extract_commit_files() {
        let cli = parse(&["peer", "extract", "commit-files", "abc123"]);

        assert_eq!(
            cli.command,
            Command::Extract {
                command: ExtractCommand::CommitFiles {
                    revision: "abc123".into()
                },
            }
        );
    }

    #[test]
    fn extract_commit_list() {
        let cli = parse(&["peer", "extract", "commit-list", "HEAD~3..HEAD"]);

        assert_eq!(
            cli.command,
            Command::Extract {
                command: ExtractCommand::CommitList {
                    range: "HEAD~3..HEAD".into()
                },
            }
        );
    }

    #[test]
    fn extract_file_content() {
        let cli = parse(&[
            "peer",
            "extract",
            "file-content",
            "abc123",
            "--path",
            "src/foo.rs",
        ]);

        assert_eq!(
            cli.command,
            Command::Extract {
                command: ExtractCommand::FileContent {
                    revision: "abc123".into(),
                    path: PathBuf::from("src/foo.rs"),
                },
            }
        );
    }

    #[test]
    fn check_size() {
        let cli = parse(&["peer", "check", "size", "abc123"]);

        assert_eq!(
            cli.command,
            Command::Check {
                provider: None,
                model: None,
                command: CheckCommand::Size {
                    revision: "abc123".into(),
                },
            }
        );
    }

    #[test]
    fn check_intent() {
        let cli = parse(&["peer", "check", "intent", "abc123"]);

        assert_eq!(
            cli.command,
            Command::Check {
                provider: None,
                model: None,
                command: CheckCommand::Intent {
                    revision: "abc123".into(),
                },
            }
        );
    }

    #[test]
    fn check_quality() {
        let cli = parse(&["peer", "check", "quality", "abc123"]);

        assert_eq!(
            cli.command,
            Command::Check {
                provider: None,
                model: None,
                command: CheckCommand::Quality {
                    revision: "abc123".into(),
                },
            }
        );
    }

    #[test]
    fn check_security() {
        let cli = parse(&["peer", "check", "security", "abc123"]);

        assert_eq!(
            cli.command,
            Command::Check {
                provider: None,
                model: None,
                command: CheckCommand::Security {
                    revision: "abc123".into(),
                },
            }
        );
    }

    #[test]
    fn check_coherence() {
        let cli = parse(&["peer", "check", "coherence", "HEAD~3..HEAD"]);

        assert_eq!(
            cli.command,
            Command::Check {
                provider: None,
                model: None,
                command: CheckCommand::Coherence {
                    range: "HEAD~3..HEAD".into(),
                },
            }
        );
    }

    #[test]
    fn check_with_provider_and_model() {
        let cli = parse(&[
            "peer",
            "check",
            "--provider",
            "anthropic",
            "--model",
            "claude-sonnet-5",
            "size",
            "abc123",
        ]);

        assert_eq!(
            cli.command,
            Command::Check {
                provider: Some("anthropic".into()),
                model: Some("claude-sonnet-5".into()),
                command: CheckCommand::Size {
                    revision: "abc123".into(),
                },
            }
        );
    }

    #[test]
    fn check_with_provider_and_model_after_check_subcommand() {
        let cli = parse(&[
            "peer",
            "check",
            "size",
            "abc123",
            "--provider",
            "anthropic",
            "--model",
            "claude-sonnet-5",
        ]);

        assert_eq!(
            cli.command,
            Command::Check {
                provider: Some("anthropic".into()),
                model: Some("claude-sonnet-5".into()),
                command: CheckCommand::Size {
                    revision: "abc123".into(),
                },
            }
        );
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
    fn render_with_json_format() {
        let cli = parse(&["peer", "render", "--format", "json"]);

        assert_eq!(
            cli.command,
            Command::Render {
                format: OutputFormat::Json,
                repo: None,
            }
        );
    }

    #[test]
    fn render_with_github_format_and_repo() {
        let cli = parse(&[
            "peer",
            "render",
            "--format",
            "github",
            "--repo",
            "sgkim126/peer",
        ]);

        assert_eq!(
            cli.command,
            Command::Render {
                format: OutputFormat::Github,
                repo: Some("sgkim126/peer".into()),
            }
        );
    }

    #[test]
    fn verbose_flag() {
        let cli = parse(&["peer", "--verbose", "init"]);

        assert!(cli.verbose);
        assert!(!cli.debug);
    }

    #[test]
    fn debug_flag() {
        let cli = parse(&["peer", "--debug", "init"]);

        assert!(!cli.verbose);
        assert!(cli.debug);
    }

    #[test]
    fn verbose_and_debug_flags() {
        let cli = parse(&["peer", "--verbose", "--debug", "init"]);

        assert!(cli.verbose);
        assert!(cli.debug);
    }

    #[test]
    fn global_flags_after_subcommand() {
        let cli = parse(&["peer", "init", "--verbose", "--debug"]);

        assert!(cli.verbose);
        assert!(cli.debug);
    }
}
