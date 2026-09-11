use std::process::{Command, Output};

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
    } else {
        config.as_table_mut().unwrap().remove("github");
    }
    std::fs::write(peer.join("config.toml"), toml::to_string(&config).unwrap()).unwrap();
    directory
}

fn command(directory: &tempfile::TempDir) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_peer"));
    command
        .current_dir(directory.path())
        .env_remove("GITHUB_TOKEN")
        .env("RUST_LOG", "debug");
    command.arg("review");
    command
}

fn assert_error(output: Output, message: &str) {
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(message), "unexpected stderr: {stderr}");
}

#[test]
fn github_rejects_missing_repository_configuration_before_authentication() {
    let directory = project(None);
    assert_error(
        command(&directory)
            .args(["--github", "123"])
            .output()
            .unwrap(),
        "--github requires [github].repo",
    );
    assert!(!directory.path().join(".peer/cache").exists());
}

#[test]
fn github_rejects_empty_repository_configuration_before_authentication() {
    let directory = project(Some(""));
    assert_error(
        command(&directory)
            .args(["--github", "123"])
            .output()
            .unwrap(),
        "--github requires [github].repo",
    );
    assert!(!directory.path().join(".peer/cache").exists());
}

#[test]
fn github_rejects_whitespace_repository_configuration_before_authentication() {
    let directory = project(Some(" \t "));
    assert_error(
        command(&directory)
            .args(["--github", "123"])
            .output()
            .unwrap(),
        "--github requires [github].repo",
    );
    assert!(!directory.path().join(".peer/cache").exists());
}

#[test]
fn github_rejects_invalid_repository_configuration() {
    let directory = project(Some("https://github.com/owner/repo"));
    assert_error(
        command(&directory)
            .args(["--github", "123"])
            .output()
            .unwrap(),
        "GitHub repository must use the form owner/name",
    );
}

#[test]
fn github_rejects_missing_token_before_reviewing() {
    let directory = project(Some("owner/repo"));
    assert_error(
        command(&directory)
            .args(["--github", "123"])
            .output()
            .unwrap(),
        "--github requires a non-empty GITHUB_TOKEN",
    );
    assert!(!directory.path().join(".peer/cache").exists());
}

#[test]
fn github_rejects_empty_token_before_reviewing() {
    let directory = project(Some("owner/repo"));
    assert_error(
        command(&directory)
            .env("GITHUB_TOKEN", "")
            .args(["--github", "123"])
            .output()
            .unwrap(),
        "--github requires a non-empty GITHUB_TOKEN",
    );
    assert!(!directory.path().join(".peer/cache").exists());
}

#[test]
fn github_rejects_whitespace_token_before_reviewing() {
    let directory = project(Some("owner/repo"));
    assert_error(
        command(&directory)
            .env("GITHUB_TOKEN", " \t")
            .args(["--github", "123"])
            .output()
            .unwrap(),
        "--github requires a non-empty GITHUB_TOKEN",
    );
    assert!(!directory.path().join(".peer/cache").exists());
}

#[test]
fn invalid_tokens_are_not_logged_even_with_debug_logging() {
    let directory = project(Some("owner/repo"));
    let output = command(&directory)
        .env("GITHUB_TOKEN", "secret-value\ninvalid")
        .args(["--github", "123"])
        .output()
        .unwrap();
    assert!(!String::from_utf8_lossy(&output.stderr).contains("secret-value"));
    assert_error(output, "GITHUB_TOKEN is not a valid HTTP bearer token");
}

#[test]
fn direct_input_does_not_require_github_configuration_or_credentials() {
    let directory = project(None);
    std::fs::write(directory.path().join("body.md"), "Description").unwrap();
    std::fs::write(directory.path().join("comments.json"), "[]").unwrap();
    let output = command(&directory)
        .args([
            "main...HEAD",
            "--title",
            "Title",
            "--body-file",
            "body.md",
            "--comments-file",
            "comments.json",
        ])
        .output()
        .unwrap();
    // Reaching the existing target validator proves context loading completed.
    assert_error(output, "main...HEAD is not a two-dot range");
}

#[test]
fn direct_input_ignores_empty_github_repository_without_credentials() {
    let directory = project(Some(""));
    std::fs::write(directory.path().join("body.md"), "Description").unwrap();
    std::fs::write(directory.path().join("comments.json"), "[]").unwrap();
    let output = command(&directory)
        .args([
            "main...HEAD",
            "--title",
            "Title",
            "--body-file",
            "body.md",
            "--comments-file",
            "comments.json",
        ])
        .output()
        .unwrap();
    // Reaching the existing target validator proves context loading completed.
    assert_error(output, "main...HEAD is not a two-dot range");
}

#[test]
fn direct_input_ignores_invalid_github_repository_without_credentials() {
    let directory = project(Some("not a repository"));
    std::fs::write(directory.path().join("body.md"), "Description").unwrap();
    std::fs::write(directory.path().join("comments.json"), "[]").unwrap();
    let output = command(&directory)
        .args([
            "main...HEAD",
            "--title",
            "Title",
            "--body-file",
            "body.md",
            "--comments-file",
            "comments.json",
        ])
        .output()
        .unwrap();
    // Reaching the existing target validator proves context loading completed.
    assert_error(output, "main...HEAD is not a two-dot range");
}

#[test]
fn github_target_conflict_fails_before_configuration_is_read() {
    let directory = tempfile::tempdir().unwrap();
    let output = command(&directory)
        .args(["HEAD", "--github", "123"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be used with"));
}

#[test]
fn github_title_conflict_fails_before_configuration_is_read() {
    let directory = tempfile::tempdir().unwrap();
    let output = command(&directory)
        .args(["--github", "123", "--title", "value"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be used with"));
}

#[test]
fn github_body_file_conflict_fails_before_configuration_is_read() {
    let directory = tempfile::tempdir().unwrap();
    let output = command(&directory)
        .args(["--github", "123", "--body-file", "value"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be used with"));
}

#[test]
fn github_comments_file_conflict_fails_before_configuration_is_read() {
    let directory = tempfile::tempdir().unwrap();
    let output = command(&directory)
        .args(["--github", "123", "--comments-file", "value"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be used with"));
}
