use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

const FINDING: &str = r#"{"commit":"abc1234","severity":"high","message":"Check this."}"#;

fn project(repository: Option<&str>) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    let peer = directory.path().join(".peer");
    std::fs::create_dir(&peer).unwrap();
    let mut config: toml::Value =
        toml::from_str(include_str!("../resources/default_config.toml")).unwrap();
    if let Some(repository) = repository {
        config["github"]
            .as_table_mut()
            .unwrap()
            .insert("repo".into(), repository.into());
    }
    std::fs::write(peer.join("config.toml"), toml::to_string(&config).unwrap()).unwrap();
    directory
}

fn malformed_project() -> tempfile::TempDir {
    let directory = project(None);
    std::fs::write(directory.path().join(".peer/config.toml"), "[[[").unwrap();
    directory
}

fn render(directory: &Path, arguments: &[&str], input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_peer"))
        .arg("render")
        .args(arguments)
        .current_dir(directory)
        .env_remove("GITHUB_TOKEN")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let _ = child.stdin.take().unwrap().write_all(input.as_bytes());
    child.wait_with_output().unwrap()
}

fn assert_rendered(output: Output, expected: &str) {
    assert!(
        output.status.success(),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains(expected), "unexpected stdout: {stdout}");
}

fn assert_error(output: Output, expected: &str) {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(expected), "unexpected stderr: {stderr}");
}

#[test]
fn github_uses_the_configured_repository_for_links() {
    let directory = project(Some("configured/repository"));
    assert_rendered(
        render(directory.path(), &["--format", "github"], FINDING),
        "https://github.com/configured/repository/commit/abc1234",
    );
}

#[test]
fn publishing_uses_repository_configuration_before_requiring_authentication() {
    let directory = project(Some("configured/repository"));
    assert_error(
        render(
            directory.path(),
            &["--format", "github", "--pr", "123"],
            FINDING,
        ),
        "GitHub access requires a non-empty GITHUB_TOKEN",
    );
}

#[test]
fn publishing_rejects_missing_repository_configuration_before_authentication() {
    let directory = project(None);
    assert_error(
        render(
            directory.path(),
            &["--format", "github", "--pr", "123"],
            FINDING,
        ),
        "--format github requires --repo <owner/name> or [github].repo",
    );
}

#[test]
fn github_discovers_repository_configuration_in_a_parent_directory() {
    let directory = project(Some("configured/repository"));
    let nested = directory.path().join("nested/deeper");
    std::fs::create_dir_all(&nested).unwrap();
    assert_rendered(
        render(&nested, &["--format", "github"], FINDING),
        "https://github.com/configured/repository/commit/abc1234",
    );
}

#[test]
fn github_repository_flag_overrides_the_configured_repository() {
    let directory = project(Some("configured/repository"));
    let output = render(
        directory.path(),
        &["--format", "github", "--repo", "override/repository"],
        FINDING,
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("configured/repository"));
    assert_rendered(
        output,
        "https://github.com/override/repository/commit/abc1234",
    );
}

#[test]
fn github_rejects_an_invalid_repository_flag_instead_of_using_configuration() {
    let directory = project(Some("configured/repository"));
    assert_error(
        render(
            directory.path(),
            &["--format", "github", "--repo", "invalid"],
            FINDING,
        ),
        "GitHub repository must use the form owner/name",
    );
}

#[test]
fn github_rejects_an_empty_repository_flag_instead_of_using_configuration() {
    let directory = project(Some("configured/repository"));
    assert_error(
        render(
            directory.path(),
            &["--format", "github", "--repo", ""],
            FINDING,
        ),
        "GitHub repository must use the form owner/name",
    );
}

#[test]
fn github_repository_flag_bypasses_malformed_configuration() {
    let directory = malformed_project();
    assert_rendered(
        render(
            directory.path(),
            &["--format", "github", "--repo", "override/repository"],
            FINDING,
        ),
        "https://github.com/override/repository/commit/abc1234",
    );
}

#[test]
fn github_without_a_repository_flag_requires_configuration() {
    let directory = tempfile::tempdir().unwrap();
    assert_error(
        render(directory.path(), &["--format", "github"], FINDING),
        "no .peer/config.toml found",
    );
}

#[test]
fn github_rejects_missing_repository_configuration() {
    let directory = project(None);
    assert_error(
        render(directory.path(), &["--format", "github"], FINDING),
        "--format github requires --repo <owner/name> or [github].repo",
    );
}

#[test]
fn github_rejects_empty_repository_configuration() {
    let directory = project(Some(""));
    assert_error(
        render(directory.path(), &["--format", "github"], FINDING),
        "--format github requires --repo <owner/name> or [github].repo",
    );
}

#[test]
fn github_rejects_whitespace_repository_configuration() {
    let directory = project(Some(" \t "));
    assert_error(
        render(directory.path(), &["--format", "github"], FINDING),
        "--format github requires --repo <owner/name> or [github].repo",
    );
}

#[test]
fn github_rejects_invalid_repository_configuration() {
    let directory = project(Some("https://github.com/configured/repository"));
    assert_error(
        render(directory.path(), &["--format", "github"], FINDING),
        "GitHub repository must use the form owner/name",
    );
}

#[test]
fn github_without_a_repository_flag_rejects_malformed_configuration() {
    let directory = malformed_project();
    assert_error(
        render(directory.path(), &["--format", "github"], FINDING),
        "invalid config in",
    );
}

#[test]
fn markdown_ignores_repository_configuration() {
    let directory = project(Some("configured/repository"));
    assert_rendered(
        render(directory.path(), &["--format", "markdown"], FINDING),
        r"Check this\.",
    );
}

#[test]
fn terminal_ignores_repository_configuration() {
    let directory = project(Some("configured/repository"));
    assert_rendered(
        render(directory.path(), &["--format", "terminal"], FINDING),
        "Check this.",
    );
}

#[test]
fn markdown_bypasses_malformed_configuration() {
    let directory = malformed_project();
    assert_rendered(
        render(directory.path(), &["--format", "markdown"], FINDING),
        r"Check this\.",
    );
}

#[test]
fn terminal_bypasses_malformed_configuration() {
    let directory = malformed_project();
    assert_rendered(
        render(directory.path(), &["--format", "terminal"], FINDING),
        "Check this.",
    );
}

#[test]
fn markdown_does_not_require_configuration() {
    let directory = tempfile::tempdir().unwrap();
    assert_rendered(
        render(directory.path(), &["--format", "markdown"], FINDING),
        r"Check this\.",
    );
}

#[test]
fn terminal_does_not_require_configuration() {
    let directory = tempfile::tempdir().unwrap();
    assert_rendered(
        render(directory.path(), &["--format", "terminal"], FINDING),
        "Check this.",
    );
}

#[test]
fn terminal_renders_model_usage_arrays_and_derives_totals_from_stages() {
    let directory = tempfile::tempdir().unwrap();
    let input = serde_json::json!({
        "summary": {"peer_version": "test"},
        "ordered_commits": ["abc1234"],
        "usage": [],
        "stages": [{
            "stage": "quality",
            "target": "abc1234",
            "outcome": {
                "status": "clean",
                "summary": "No issues.",
                "iterations": 2,
                "usage": [
                    {
                        "provider": "first",
                        "model": "shared",
                        "input_tokens": 10,
                        "output_tokens": 2,
                        "cache_read_tokens": 0,
                        "cache_write_tokens": 0,
                        "cost_usd": 0.125
                    }, {
                        "provider": "second",
                        "model": "shared",
                        "input_tokens": 20,
                        "output_tokens": 4,
                        "cache_read_tokens": 0,
                        "cache_write_tokens": 0,
                        "cost_usd": 0.25
                    }
                ]
            }
        }]
    });
    let output = render(
        directory.path(),
        &["--format", "terminal"],
        &input.to_string(),
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.contains("  - first/shared: 10 input, 2 output, $0.125000"));
    assert!(stdout.contains("  - second/shared: 20 input, 4 output, $0.250000"));
}

#[test]
fn markdown_rejects_a_repository_flag_before_parsing_input() {
    let directory = malformed_project();
    assert_error(
        render(
            directory.path(),
            &["--format", "markdown", "--repo", "owner/repository"],
            "invalid",
        ),
        "--repo can only be used with --format github",
    );
}

#[test]
fn terminal_rejects_a_repository_flag_before_parsing_input() {
    let directory = malformed_project();
    assert_error(
        render(
            directory.path(),
            &["--format", "terminal", "--repo", "owner/repository"],
            "invalid",
        ),
        "--repo can only be used with --format github",
    );
}
