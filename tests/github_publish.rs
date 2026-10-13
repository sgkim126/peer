use std::assert_matches;
use std::io::Write;
use std::process::{Command, Output, Stdio};

mod support;

fn render(arguments: &[&str], input: &str) -> Output {
    let directory = tempfile::tempdir().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_peer"));
    support::configure_gh(&mut command, directory.path(), true);
    let mut child = command
        .arg("render")
        .args(arguments)
        .current_dir(directory.path())
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
fn github_output_without_publishing_does_not_require_authentication_or_a_checkout() {
    let output = render(&["--format", "github", "--repo", "owner/repo"], FINDING);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("**finding/high**"));
}

#[test]
fn publishing_reports_gh_authentication_failure_without_discovering_project_configuration() {
    let output = render(
        &[
            "--format",
            "github",
            "--repo",
            "owner/repo",
            "--github",
            "123",
        ],
        FINDING,
    );
    assert_eq!(output.status.code(), Some(1));
    assert_matches!(output.stdout[..], []);
    assert!(String::from_utf8_lossy(&output.stderr).contains("gh command failed"));
}

#[test]
fn publishing_rejects_markdown_before_reading_input() {
    let output = render(
        &[
            "--format",
            "markdown",
            "--repo",
            "owner/repo",
            "--github",
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
            "--github",
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
    let output = render(&["--format", "markdown", "--github", "123"], "invalid");
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("--github can only be used with --format github")
    );
}

#[test]
fn publishing_without_repo_rejects_terminal_before_reading_input() {
    let output = render(&["--format", "terminal", "--github", "123"], "invalid");
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("--github can only be used with --format github")
    );
}

#[test]
fn publishing_with_default_format_reports_gh_authentication_failure() {
    let output = render(&["--github", "123", "--repo", "owner/repo"], FINDING);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("gh command failed"));
}

#[test]
fn publishing_requires_gh_before_reading_stdin_or_configuration() {
    let directory = tempfile::tempdir().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_peer"));
    support::configure_gh(&mut command, directory.path(), false);
    let mut child = command
        .args(["render", "--github", "123"])
        .current_dir(directory.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Keep stdin open: returning proves the availability check precedes reading it.
    let stdin = child.stdin.take().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("render waited for stdin before checking gh availability");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--github requires GitHub CLI (gh)"),
        "{stderr}"
    );
    assert!(stderr.contains("PATH"), "{stderr}");
    assert!(!directory.path().join(".peer").exists());
}
