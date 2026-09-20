use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

const FINDING: &str = r#"{
    "ordered_commits": ["abc1234"],
    "findings": [{"commit":"abc1234","severity":"high","message":"Check this."}],
    "stages": []
}"#;

fn render(directory: &Path, arguments: &[&str]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_peer"))
        .arg("render")
        .args(arguments)
        .current_dir(directory)
        .env_remove("GITLAB_TOKEN")
        .env_remove("GITHUB_TOKEN")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let _ = child.stdin.take().unwrap().write_all(FINDING.as_bytes());
    child.wait_with_output().unwrap()
}

fn assert_rendered(output: Output, repository: &str) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = String::from_utf8(output.stdout).unwrap();
    assert!(output.contains(&format!("https://gitlab.com/{repository}/-/commit/abc1234")));
    assert!(!output.contains("github.com"));
}

#[test]
fn explicit_repository_renders_without_configuration_authentication_or_checkout() {
    let directory = tempfile::tempdir().unwrap();
    assert_rendered(
        render(
            directory.path(),
            &["--format", "gitlab", "--repo", "group/subgroup/project"],
        ),
        "group/subgroup/project",
    );
}

#[test]
fn gitlab_uses_only_its_own_configured_repository() {
    let directory = tempfile::tempdir().unwrap();
    let peer = directory.path().join(".peer");
    std::fs::create_dir(&peer).unwrap();
    let mut config: toml::Value =
        toml::from_str(include_str!("../resources/default_config.toml")).unwrap();
    config["github"]
        .as_table_mut()
        .unwrap()
        .insert("repo".into(), "github/other-project".into());
    config["gitlab"] = toml::Value::Table(toml::Table::from_iter([(
        "repo".into(),
        "group/subgroup/configured".into(),
    )]));
    std::fs::write(peer.join("config.toml"), toml::to_string(&config).unwrap()).unwrap();
    assert_rendered(
        render(directory.path(), &["--format", "gitlab"]),
        "group/subgroup/configured",
    );
    assert_rendered(
        render(
            directory.path(),
            &["--format", "gitlab", "--repo", "group/override"],
        ),
        "group/override",
    );
}

#[test]
fn explicit_repository_bypasses_malformed_configuration() {
    let directory = tempfile::tempdir().unwrap();
    let peer = directory.path().join(".peer");
    std::fs::create_dir(&peer).unwrap();
    std::fs::write(peer.join("config.toml"), "[[[").unwrap();
    assert_rendered(
        render(
            directory.path(),
            &["--format", "gitlab", "--repo", "group/project"],
        ),
        "group/project",
    );
}

#[test]
fn gitlab_without_a_repository_flag_requires_configuration() {
    let directory = tempfile::tempdir().unwrap();
    let output = render(directory.path(), &["--format", "gitlab"]);

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("no .peer/config.toml found"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn gitlab_rejects_missing_repository_configuration() {
    let directory = tempfile::tempdir().unwrap();
    let peer = directory.path().join(".peer");
    std::fs::create_dir(&peer).unwrap();
    let mut config: toml::Value =
        toml::from_str(include_str!("../resources/default_config.toml")).unwrap();
    config["gitlab"].as_table_mut().unwrap().remove("repo");
    std::fs::write(peer.join("config.toml"), toml::to_string(&config).unwrap()).unwrap();

    let output = render(directory.path(), &["--format", "gitlab"]);

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains(
            "--format gitlab requires --repo <namespace/project> or [gitlab].repo in .peer/config.toml"
        ),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn gitlab_rejects_repository_paths_with_empty_components() {
    let directory = tempfile::tempdir().unwrap();
    let output = render(
        directory.path(),
        &["--format", "gitlab", "--repo", "group//project"],
    );

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains(
            "GitLab repository must use the form namespace/project (subgroups are supported)"
        ),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn gitlab_rejects_repository_urls() {
    let directory = tempfile::tempdir().unwrap();
    let output = render(
        directory.path(),
        &[
            "--format",
            "gitlab",
            "--repo",
            "https://gitlab.com/group/project",
        ],
    );

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains(
            "GitLab repository must use the form namespace/project (subgroups are supported)"
        ),
        "unexpected stderr: {stderr}"
    );
}
