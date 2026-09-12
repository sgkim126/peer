use std::io::Write;
use std::process::{Command, Output, Stdio};

fn render(arguments: &[&str], input: &str) -> Output {
    let directory = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_peer"))
        .arg("render")
        .args(arguments)
        .current_dir(directory.path())
        .env_remove("GITHUB_TOKEN")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let _ = child.stdin.take().unwrap().write_all(input.as_bytes());
    child.wait_with_output().unwrap()
}

const FINDING: &str = r#"{"commit":"abc1234","severity":"high","message":"Check this."}"#;

#[test]
fn github_output_without_pr_does_not_require_authentication_or_a_checkout() {
    let output = render(&["--format", "github", "--repo", "owner/repo"], FINDING);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("**finding/high**"));
}

#[test]
fn publishing_requires_a_token_without_discovering_project_configuration() {
    let output = render(
        &["--format", "github", "--repo", "owner/repo", "--pr", "123"],
        FINDING,
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("GitHub access requires a non-empty GITHUB_TOKEN")
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
            "--pr",
            "123",
        ],
        "invalid",
    );
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("--repo can only be used with --format github")
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
            "--pr",
            "123",
        ],
        "invalid",
    );
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("--repo can only be used with --format github")
    );
}
