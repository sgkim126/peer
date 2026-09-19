use std::io::Write;
use std::process::{Command, Output, Stdio};

fn render(arguments: &[&str], input: &str) -> Output {
    let directory = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_peer"))
        .arg("render")
        .args(arguments)
        .current_dir(directory.path())
        .env_remove("GITLAB_TOKEN")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let _ = child.stdin.take().unwrap().write_all(input.as_bytes());
    child.wait_with_output().unwrap()
}

const FINDING: &str = r#"{
    "ordered_commits": ["abc1234"],
    "findings": [{"commit":"abc1234","severity":"high","message":"Check this."}],
    "stages": []
}"#;

#[test]
fn gitlab_output_without_publishing_does_not_require_authentication_or_a_checkout() {
    let output = render(&["--format", "gitlab", "--repo", "owner/repo"], FINDING);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("**finding/high**"));
}

#[test]
fn publishing_requires_a_token_without_discovering_project_configuration() {
    let output = render(
        &[
            "--format",
            "gitlab",
            "--repo",
            "owner/repo",
            "--gitlab",
            "123",
        ],
        FINDING,
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("GitLab access requires a non-empty GITLAB_TOKEN")
    );
}

#[test]
fn publishing_rejects_markdown_before_reading_input() {
    let output = render(
        &[
            "--format",
            "markdown",
            "--repo",
            "owner/repo",
            "--gitlab",
            "123",
        ],
        "invalid",
    );
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("--repo can only be used with --format github or --format gitlab")
    );
}

#[test]
fn publishing_rejects_terminal_before_reading_input() {
    let output = render(
        &[
            "--format",
            "terminal",
            "--repo",
            "owner/repo",
            "--gitlab",
            "123",
        ],
        "invalid",
    );
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("--repo can only be used with --format github or --format gitlab")
    );
}

#[test]
fn publishing_without_repo_rejects_markdown_before_reading_input() {
    let output = render(&["--format", "markdown", "--gitlab", "123"], "invalid");
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("--gitlab can only be used with --format gitlab")
    );
}

#[test]
fn publishing_without_repo_rejects_terminal_before_reading_input() {
    let output = render(&["--format", "terminal", "--gitlab", "123"], "invalid");
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("--gitlab can only be used with --format gitlab")
    );
}

#[test]
fn publishing_with_default_format_requires_a_token() {
    let output = render(&["--gitlab", "123", "--repo", "owner/repo"], FINDING);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("GitLab access requires a non-empty GITLAB_TOKEN")
    );
}

#[test]
fn gitlab_rejects_github_format_before_parsing_input() {
    let output = render(
        &[
            "--gitlab",
            "123",
            "--format",
            "github",
            "--repo",
            "group/project",
        ],
        "invalid",
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("--gitlab can only be used with --format gitlab")
    );
}

#[test]
fn publishing_services_are_mutually_exclusive_before_configuration() {
    let output = render(&["--gitlab", "123", "--github", "456"], "invalid");
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be used with"));
}
