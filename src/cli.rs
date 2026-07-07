use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

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

    Review {
        target: String,

        #[arg(long)]
        title: Option<String>,

        #[arg(long)]
        body_file: Option<PathBuf>,

        #[arg(long)]
        comments_file: Option<PathBuf>,

        #[arg(long, default_value = "terminal")]
        format: OutputFormat,
    },

    Extract {
        #[command(subcommand)]
        command: ExtractCommand,
    },

    Check {
        #[command(subcommand)]
        command: CheckCommand,
    },

    Render {
        #[arg(long, default_value = "terminal")]
        format: OutputFormat,
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
    fn review_with_default_format() {
        let cli = parse(&["peer", "review", "HEAD~3..HEAD"]);

        assert_eq!(
            cli.command,
            Command::Review {
                target: "HEAD~3..HEAD".into(),
                title: None,
                body_file: None,
                comments_file: None,
                format: OutputFormat::Terminal,
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
                title: None,
                body_file: None,
                comments_file: None,
                format: OutputFormat::Json,
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
                title: None,
                body_file: None,
                comments_file: None,
                format: OutputFormat::Markdown,
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
                title: Some("Add review context".into()),
                body_file: Some(PathBuf::from("body.md")),
                comments_file: Some(PathBuf::from("comments.json")),
                format: OutputFormat::Terminal,
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
                command: CheckCommand::Coherence {
                    range: "HEAD~3..HEAD".into(),
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
                format: OutputFormat::Terminal
            }
        );
    }

    #[test]
    fn render_with_json_format() {
        let cli = parse(&["peer", "render", "--format", "json"]);

        assert_eq!(
            cli.command,
            Command::Render {
                format: OutputFormat::Json
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
