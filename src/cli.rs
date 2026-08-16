use std::num::NonZeroU8;
use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

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

        /// Start resumable checks from the beginning.
        #[arg(long)]
        no_resume: bool,

        #[arg(long, default_value = "terminal")]
        format: OutputFormat,

        #[arg(long, required_if_eq("format", "github"))]
        repo: Option<String>,
    },

    Extract {
        #[command(subcommand)]
        command: ExtractCommand,
    },

    Render {
        #[arg(long, default_value = "terminal")]
        format: OutputFormat,

        #[arg(long, required_if_eq("format", "github"))]
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
    FileDiff {
        from_revision: String,
        to_revision: String,
        #[arg(long)]
        path: PathBuf,
    },
    ListTree {
        revision: String,
        #[arg(long)]
        path: Option<PathBuf>,
        #[arg(long)]
        recursive: bool,
    },
    Grep {
        revision: String,
        query: String,
        #[arg(long)]
        path: Option<PathBuf>,
        #[arg(long, default_value = "2")]
        context_lines: NonZeroU8,
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

    use std::assert_matches;

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

        assert_eq!(cli.command, Command::Prune { all: false });
    }

    #[test]
    fn prune_all() {
        let cli = parse(&["peer", "prune", "--all"]);

        assert_eq!(cli.command, Command::Prune { all: true });
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
                skip_checks: Vec::new(),
                only_checks: Vec::new(),
                title: None,
                body_file: None,
                comments_file: None,
                no_resume: false,
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
                skip_checks: Vec::new(),
                only_checks: Vec::new(),
                title: None,
                body_file: None,
                comments_file: None,
                no_resume: false,
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
                skip_checks: Vec::new(),
                only_checks: Vec::new(),
                title: None,
                body_file: None,
                comments_file: None,
                no_resume: false,
                format: OutputFormat::Markdown,
                repo: None,
            }
        );
    }

    #[test]
    fn review_with_github_format_requires_repo() {
        let result = Cli::try_parse_from(["peer", "review", "HEAD", "--format", "github"]);

        assert_matches!(result, Err(_));
    }

    #[test]
    fn review_with_github_format_and_repo() {
        let cli = parse(&[
            "peer",
            "review",
            "HEAD",
            "--format",
            "github",
            "--repo",
            "owner/repository",
        ]);

        assert_eq!(
            cli.command,
            Command::Review {
                target: "HEAD".into(),
                provider: None,
                model: None,
                skip_checks: Vec::new(),
                only_checks: Vec::new(),
                title: None,
                body_file: None,
                comments_file: None,
                no_resume: false,
                format: OutputFormat::Github,
                repo: Some("owner/repository".into()),
            }
        );
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
                skip_checks: Vec::new(),
                only_checks: Vec::new(),
                title: Some("Add context compression".into()),
                body_file: Some("body.md".into()),
                comments_file: Some("comments.json".into()),
                no_resume: false,
                format: OutputFormat::Terminal,
                repo: None,
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
                skip_checks: Vec::new(),
                only_checks: Vec::new(),
                title: None,
                body_file: None,
                comments_file: None,
                no_resume: false,
                format: OutputFormat::Terminal,
                repo: None,
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
    fn review_with_only_checks() {
        let cli = parse(&[
            "peer",
            "review",
            "HEAD~2..HEAD",
            "--only-check",
            "intent,coherence",
        ]);

        assert_eq!(
            cli.command,
            Command::Review {
                target: "HEAD~2..HEAD".into(),
                provider: None,
                model: None,
                skip_checks: Vec::new(),
                only_checks: vec![ReviewCheckKind::Intent, ReviewCheckKind::Coherence],
                title: None,
                body_file: None,
                comments_file: None,
                no_resume: false,
                format: OutputFormat::Terminal,
                repo: None,
            }
        );
    }

    #[test]
    fn review_with_skipped_checks() {
        let cli = parse(&["peer", "review", "HEAD", "--skip-check", "quality,security"]);

        assert_eq!(
            cli.command,
            Command::Review {
                target: "HEAD".into(),
                provider: None,
                model: None,
                skip_checks: vec![ReviewCheckKind::Quality, ReviewCheckKind::Security],
                only_checks: Vec::new(),
                title: None,
                body_file: None,
                comments_file: None,
                no_resume: false,
                format: OutputFormat::Terminal,
                repo: None,
            }
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
    fn review_rejects_only_and_skip_checks_together() {
        let result = Cli::try_parse_from([
            "peer",
            "review",
            "HEAD",
            "--only-check",
            "intent",
            "--skip-check",
            "security",
        ]);

        assert_matches!(result, Err(_));
    }

    #[test]
    fn review_rejects_only_check_without_a_value() {
        let result = Cli::try_parse_from(["peer", "review", "HEAD", "--only-check"]);

        assert_matches!(result, Err(_));
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
    fn extract_file_diff() {
        let cli = parse(&[
            "peer",
            "extract",
            "file-diff",
            "HEAD~1",
            "HEAD",
            "--path",
            "src/foo.rs",
        ]);

        assert_eq!(
            cli.command,
            Command::Extract {
                command: ExtractCommand::FileDiff {
                    from_revision: "HEAD~1".into(),
                    to_revision: "HEAD".into(),
                    path: PathBuf::from("src/foo.rs"),
                },
            }
        );
    }

    #[test]
    fn extract_list_tree() {
        let cli = parse(&[
            "peer",
            "extract",
            "list-tree",
            "HEAD",
            "--path",
            "src",
            "--recursive",
        ]);

        assert_eq!(
            cli.command,
            Command::Extract {
                command: ExtractCommand::ListTree {
                    revision: "HEAD".into(),
                    path: Some(PathBuf::from("src")),
                    recursive: true,
                },
            }
        );
    }

    #[test]
    fn extract_grep() {
        let cli = parse(&[
            "peer",
            "extract",
            "grep",
            "HEAD",
            "validate_token",
            "--path",
            "src",
            "--context-lines",
            "2",
        ]);

        assert_eq!(
            cli.command,
            Command::Extract {
                command: ExtractCommand::Grep {
                    revision: "HEAD".into(),
                    query: "validate_token".into(),
                    path: Some(PathBuf::from("src")),
                    context_lines: NonZeroU8::new(2).unwrap(),
                },
            }
        );
    }

    #[test]
    fn extract_grep_defaults_to_two_context_lines() {
        let cli = parse(&["peer", "extract", "grep", "HEAD", "query"]);

        assert_matches!(
            cli.command,
            Command::Extract {
                command: ExtractCommand::Grep {
                    context_lines,
                    ..
                },
            } if context_lines == NonZeroU8::new(2).unwrap()
        );
    }

    #[test]
    fn extract_grep_rejects_zero_context_lines() {
        let result = Cli::try_parse_from([
            "peer",
            "extract",
            "grep",
            "HEAD",
            "query",
            "--context-lines",
            "0",
        ]);

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
