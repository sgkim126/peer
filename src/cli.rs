use std::num::NonZeroU64;
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
    Init {
        /// Set llm.default_provider in the generated config.
        #[arg(long)]
        provider: Option<String>,

        /// Set llm.default_model in the generated config.
        #[arg(long)]
        model: Option<String>,

        /// Set the selected service's repository in the generated config (defaults to GitHub).
        #[arg(long, value_name = "NAMESPACE/PROJECT")]
        repo: Option<String>,

        /// Set github.repo with --repo (default).
        #[arg(long, conflicts_with = "gitlab", requires = "repo")]
        github: bool,

        /// Set gitlab.repo with --repo.
        #[arg(long, conflicts_with = "github", requires = "repo")]
        gitlab: bool,
    },

    /// Remove cached values.
    Prune {
        /// Remove all cached values, including values for the current version.
        #[arg(long)]
        all: bool,
    },

    #[command(group(clap::ArgGroup::new("remote_review").args(["github", "gitlab"])))]
    Review {
        #[arg(required_unless_present_any = ["github", "gitlab"])]
        target: Option<String>,

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

        /// Review the commits and context from a GitHub pull request.
        #[arg(long, value_name = "PR_NUMBER", conflicts_with_all = ["gitlab", "target", "title", "body_file", "comments_file"])]
        github: Option<NonZeroU64>,

        /// Review the commits and context from a GitLab.com merge request.
        #[arg(long, value_name = "MR_IID", conflicts_with_all = ["github", "target", "title", "body_file", "comments_file"])]
        gitlab: Option<NonZeroU64>,

        /// Override the selected service's repository for this review.
        #[arg(
            long,
            value_name = "NAMESPACE/PROJECT",
            requires = "remote_review",
            conflicts_with = "target"
        )]
        repo: Option<String>,

        /// Start resumable stages from the beginning.
        #[arg(long)]
        no_resume: bool,
    },

    Render {
        /// Output format (defaults to the selected publishing service).
        #[arg(
            long,
            default_value = "terminal",
            default_value_if("github", clap::builder::ArgPredicate::IsPresent, "github"),
            default_value_if("gitlab", clap::builder::ArgPredicate::IsPresent, "gitlab")
        )]
        format: OutputFormat,

        /// Override the repository for GitHub- or GitLab-formatted output.
        #[arg(long, value_name = "NAMESPACE/PROJECT")]
        repo: Option<String>,

        /// Publish the review to this GitHub pull request (defaults --format to github).
        #[arg(long, value_name = "PR_NUMBER", conflicts_with = "gitlab")]
        github: Option<NonZeroU64>,

        /// Publish the review to this GitLab.com merge request (defaults --format to gitlab).
        #[arg(long, value_name = "MR_IID", conflicts_with = "github")]
        gitlab: Option<NonZeroU64>,
    },
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq)]
pub enum OutputFormat {
    Terminal,
    Markdown,
    Github,
    Gitlab,
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

        assert_eq!(
            cli.command,
            Command::Init {
                provider: None,
                model: None,
                repo: None,
                github: false,
                gitlab: false,
            }
        );
    }

    #[test]
    fn init_with_config_overrides() {
        let cli = parse(&[
            "peer",
            "init",
            "--provider",
            "custom",
            "--model",
            "namespace/model",
            "--repo",
            "owner/repository",
        ]);

        assert_eq!(
            cli.command,
            Command::Init {
                provider: Some("custom".into()),
                model: Some("namespace/model".into()),
                repo: Some("owner/repository".into()),
                github: false,
                gitlab: false,
            }
        );
    }

    #[test]
    fn init_github_requires_repo() {
        let error = Cli::try_parse_from(["peer", "init", "--github"]).unwrap_err();

        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::MissingRequiredArgument
        );
    }

    #[test]
    fn init_gitlab_requires_repo() {
        let error = Cli::try_parse_from(["peer", "init", "--gitlab"]).unwrap_err();

        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::MissingRequiredArgument
        );
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
                target: Some("HEAD~3..HEAD".into()),
                provider: None,
                model: None,
                title: None,
                body_file: None,
                comments_file: None,
                github: None,
                gitlab: None,
                repo: None,
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
                target: Some("HEAD".into()),
                provider: None,
                model: None,
                title: Some("Add context compression".into()),
                body_file: Some("body.md".into()),
                comments_file: Some("comments.json".into()),
                github: None,
                gitlab: None,
                repo: None,
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
                target: Some("HEAD".into()),
                provider: Some("openai".into()),
                model: Some("gpt-5.6-terra".into()),
                title: None,
                body_file: None,
                comments_file: None,
                github: None,
                gitlab: None,
                repo: None,
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
    fn review_accepts_a_github_pull_request_number() {
        let cli = parse(&["peer", "review", "--github", "123"]);
        assert_matches!(cli.command, Command::Review { target: None, github: Some(number), .. } if number.get() == 123);
    }

    #[test]
    fn review_accepts_gitlab_and_a_subgroup_repository() {
        let cli = parse(&[
            "peer",
            "review",
            "--gitlab",
            "123",
            "--repo",
            "group/subgroup/project",
        ]);
        assert_matches!(cli.command, Command::Review { target: None, github: None, gitlab: Some(number), repo: Some(repo), .. } if number.get() == 123 && repo == "group/subgroup/project");
    }

    #[test]
    fn gitlab_review_conflicts_with_a_target() {
        let error = Cli::try_parse_from(["peer", "review", "--gitlab", "123", "HEAD"]).unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn gitlab_review_conflicts_with_github() {
        let error = Cli::try_parse_from(["peer", "review", "--gitlab", "123", "--github", "1"])
            .unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn gitlab_review_conflicts_with_title() {
        let error = Cli::try_parse_from(["peer", "review", "--gitlab", "123", "--title", "title"])
            .unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn gitlab_review_conflicts_with_body_file() {
        let error = Cli::try_parse_from([
            "peer",
            "review",
            "--gitlab",
            "123",
            "--body-file",
            "body.md",
        ])
        .unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn gitlab_review_conflicts_with_comments_file() {
        let error = Cli::try_parse_from([
            "peer",
            "review",
            "--gitlab",
            "123",
            "--comments-file",
            "comments.json",
        ])
        .unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn gitlab_review_rejects_zero_merge_request_iid() {
        assert_matches!(
            Cli::try_parse_from(["peer", "review", "--gitlab", "0"]),
            Err(_)
        );
    }

    #[test]
    fn gitlab_review_rejects_negative_merge_request_iid() {
        assert_matches!(
            Cli::try_parse_from(["peer", "review", "--gitlab", "-1"]),
            Err(_)
        );
    }

    #[test]
    fn gitlab_review_rejects_nonnumeric_merge_request_iid() {
        assert_matches!(
            Cli::try_parse_from(["peer", "review", "--gitlab", "abc"]),
            Err(_)
        );
    }

    #[test]
    fn gitlab_review_rejects_fractional_merge_request_iid() {
        assert_matches!(
            Cli::try_parse_from(["peer", "review", "--gitlab", "1.5"]),
            Err(_)
        );
    }

    #[test]
    fn gitlab_review_requires_a_merge_request_iid() {
        assert_matches!(Cli::try_parse_from(["peer", "review", "--gitlab"]), Err(_));
    }

    #[test]
    fn review_repository_override_requires_a_remote_review() {
        assert_matches!(
            Cli::try_parse_from(["peer", "review", "--repo", "group/project"]),
            Err(_)
        );
    }

    #[test]
    fn review_accepts_a_github_repository_override() {
        let cli = parse(&["peer", "review", "--github", "123", "--repo", "owner/repo"]);
        assert_matches!(cli.command, Command::Review { repo: Some(repo), .. } if repo == "owner/repo");
    }

    #[test]
    fn review_repository_override_requires_github() {
        assert_matches!(
            Cli::try_parse_from(["peer", "review", "HEAD", "--repo", "owner/repo"]),
            Err(_)
        );
    }

    #[test]
    fn github_conflicts_with_a_target_before_the_option() {
        let error = Cli::try_parse_from(["peer", "review", "HEAD", "--github", "123"]).unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn github_conflicts_with_a_target_after_the_option() {
        let error = Cli::try_parse_from(["peer", "review", "--github", "123", "HEAD"]).unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn github_conflicts_with_each_direct_context_option_in_either_order() {
        for option in ["--title", "--body-file", "--comments-file"] {
            for args in [
                vec!["peer", "review", "--github", "123", option, "value"],
                vec!["peer", "review", option, "value", "--github", "123"],
            ] {
                let error = Cli::try_parse_from(args).unwrap_err();
                assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
            }
        }
    }

    #[test]
    fn github_rejects_zero_pull_request_number() {
        assert!(Cli::try_parse_from(["peer", "review", "--github", "0"]).is_err());
    }

    #[test]
    fn github_rejects_negative_pull_request_number() {
        assert!(Cli::try_parse_from(["peer", "review", "--github", "-1"]).is_err());
    }

    #[test]
    fn github_rejects_nonnumeric_pull_request_number() {
        assert!(Cli::try_parse_from(["peer", "review", "--github", "abc"]).is_err());
    }

    #[test]
    fn github_rejects_fractional_pull_request_number() {
        assert!(Cli::try_parse_from(["peer", "review", "--github", "1.5"]).is_err());
    }

    #[test]
    fn github_requires_a_pull_request_number() {
        assert!(Cli::try_parse_from(["peer", "review", "--github"]).is_err());
    }

    #[test]
    fn review_requires_a_target_without_github() {
        let error = Cli::try_parse_from(["peer", "review"]).unwrap_err();
        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::MissingRequiredArgument
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
                github: None,
                gitlab: None,
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
                github: None,
                gitlab: None,
            }
        );
    }

    #[test]
    fn render_with_github_format_accepts_an_omitted_repository() {
        let cli = parse(&["peer", "render", "--format", "github"]);

        assert_matches!(
            cli.command,
            Command::Render {
                format: OutputFormat::Github,
                repo: None,
                ..
            }
        );
    }

    #[test]
    fn render_accepts_a_positive_pull_request_number() {
        let cli = parse(&[
            "peer",
            "render",
            "--format",
            "github",
            "--repo",
            "owner/repo",
            "--github",
            "123",
        ]);
        assert_matches!(cli.command, Command::Render { github: Some(number), .. } if number.get() == 123);
    }

    #[test]
    fn render_gitlab_defaults_its_format_and_rejects_other_publishers() {
        let cli = parse(&["peer", "render", "--gitlab", "123"]);
        assert_matches!(cli.command, Command::Render { format: OutputFormat::Gitlab, github: None, gitlab: Some(number), .. } if number.get() == 123);
        let error = Cli::try_parse_from(["peer", "render", "--gitlab", "123", "--github", "456"])
            .unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
        for value in ["0", "-1", "abc", "1.5"] {
            assert_matches!(
                Cli::try_parse_from(["peer", "render", "--gitlab", value]),
                Err(_)
            );
        }
        assert_matches!(Cli::try_parse_from(["peer", "render", "--gitlab"]), Err(_));
    }

    #[test]
    fn render_rejects_zero_pull_request_number() {
        assert_matches!(
            Cli::try_parse_from([
                "peer",
                "render",
                "--format",
                "github",
                "--repo",
                "owner/repo",
                "--github",
                "0",
            ]),
            Err(_)
        );
    }

    #[test]
    fn render_rejects_negative_pull_request_number() {
        assert_matches!(
            Cli::try_parse_from([
                "peer",
                "render",
                "--format",
                "github",
                "--repo",
                "owner/repo",
                "--github",
                "-1",
            ]),
            Err(_)
        );
    }

    #[test]
    fn render_rejects_nonnumeric_pull_request_number() {
        assert_matches!(
            Cli::try_parse_from([
                "peer",
                "render",
                "--format",
                "github",
                "--repo",
                "owner/repo",
                "--github",
                "abc",
            ]),
            Err(_)
        );
    }

    #[test]
    fn render_with_pull_request_accepts_an_omitted_repository() {
        let cli = parse(&["peer", "render", "--format", "github", "--github", "123"]);
        assert_matches!(
            cli.command,
            Command::Render { repo: None, github: Some(number), .. } if number.get() == 123
        );
    }

    #[test]
    fn render_with_pull_request_defaults_to_github_format() {
        let cli = parse(&["peer", "render", "--github", "123"]);
        assert_matches!(
            cli.command,
            Command::Render {
                format: OutputFormat::Github,
                repo: None,
                github: Some(number),
                gitlab: None,
            } if number.get() == 123
        );
    }
}
