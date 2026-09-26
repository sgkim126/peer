use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

const DEFAULT_CONFIG_TOML: &str = include_str!("../resources/default_config.toml");

#[must_use]
fn peer_in(tmp: &TempDir) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_peer"));
    cmd.current_dir(tmp.path());
    cmd
}

#[must_use]
fn peer_in_tmp() -> (Command, TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let cmd = peer_in(&tmp);
    (cmd, tmp)
}

fn git_init(path: &Path) {
    Command::new("git")
        .args(["init"])
        .current_dir(path)
        .output()
        .unwrap()
        .assert_success();
}

fn assert_init_overrides(
    provider: Option<&str>,
    model: Option<&str>,
    repo: Option<&str>,
    selector: Option<&str>,
) -> String {
    let (mut cmd, tmp) = peer_in_tmp();
    git_init(tmp.path());
    cmd.arg("init");
    if let Some(selector) = selector {
        cmd.arg(selector);
    }
    let repository_section = if selector == Some("--gitlab") {
        "gitlab"
    } else {
        "github"
    };
    let mut expected: toml::Value = toml::from_str(DEFAULT_CONFIG_TOML).unwrap();
    for (flag, section, key, value) in [
        ("--provider", "llm", "default_provider", provider),
        ("--model", "llm", "default_model", model),
        ("--repo", repository_section, "repo", repo),
    ] {
        if let Some(value) = value {
            cmd.args([flag, value]);
            expected[section]
                .as_table_mut()
                .unwrap()
                .insert(key.into(), value.into());
        }
    }
    cmd.output().unwrap().assert_success();

    let content = std::fs::read_to_string(tmp.path().join(".peer/config.toml")).unwrap();
    let actual: toml::Value = toml::from_str(&content).unwrap();
    assert_eq!(actual, expected);
    content
}

fn assert_init_fails_when_peer_already_exists(args: &[&str]) {
    let (mut cmd, tmp) = peer_in_tmp();
    git_init(tmp.path());
    cmd.arg("init").output().unwrap().assert_success();

    let config_path = tmp.path().join(".peer/config.toml");
    let gitignore_path = tmp.path().join(".peer/.gitignore");
    std::fs::write(&config_path, "existing config\n").unwrap();
    std::fs::write(&gitignore_path, "keep-this/\n").unwrap();

    let out = peer_in(&tmp).arg("init").args(args).output().unwrap();
    assert!(!out.status.success());
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        std::fs::read_to_string(&config_path).unwrap(),
        "existing config\n"
    );
    assert_eq!(
        std::fs::read_to_string(&gitignore_path).unwrap(),
        "keep-this/\n"
    );
}

fn assert_init_overrides_preserve_comments(
    provider: Option<&str>,
    model: Option<&str>,
    repo: Option<&str>,
    selector: Option<&str>,
) {
    let content = assert_init_overrides(provider, model, repo, selector);
    let comments_and_sections = |text: &str| {
        text.lines()
            .filter(|line| line.starts_with('#') || line.starts_with('['))
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let mut expected_comments_and_sections = comments_and_sections(DEFAULT_CONFIG_TOML);
    if repo.is_some() {
        let repository_example = if selector == Some("--gitlab") {
            "# repo = \"group/subgroup/project\" # Set it to use --gitlab on GitLab.com."
        } else {
            "# repo = \"owner/repository\" # Set it to use --github."
        };
        expected_comments_and_sections.retain(|line| line != repository_example);
    }
    assert_eq!(
        comments_and_sections(&content),
        expected_comments_and_sections
    );
}

trait OutputExt {
    fn assert_success(&self);
}

impl OutputExt for std::process::Output {
    fn assert_success(&self) {
        assert!(
            self.status.success(),
            "command failed with status {:?}\nstderr: {}",
            self.status.code(),
            String::from_utf8_lossy(&self.stderr)
        );
    }
}

#[test]
fn init_fails_without_git_repo() {
    let (mut cmd, _tmp) = peer_in_tmp();
    let out = cmd.arg("init").output().unwrap();

    assert!(!out.status.success());
    assert_eq!(out.status.code(), Some(1));
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn init_fails_when_git_is_unavailable() {
    let (mut cmd, tmp) = peer_in_tmp();
    let out = cmd.env("PATH", "").arg("init").output().unwrap();

    assert!(!out.status.success());
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("cannot run git"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(!tmp.path().join(".peer").exists());
}

#[test]
fn init_succeeds_in_git_repo() {
    let (mut cmd, tmp) = peer_in_tmp();
    git_init(tmp.path());

    let out = cmd.arg("init").output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert_eq!(
        std::fs::read_to_string(tmp.path().join(".peer").join("config.toml")).unwrap(),
        DEFAULT_CONFIG_TOML
    );
    assert_eq!(
        std::fs::read_to_string(tmp.path().join(".peer").join(".gitignore")).unwrap(),
        "cache/\n"
    );
}

#[test]
fn init_fails_when_peer_already_exists_without_overrides() {
    assert_init_fails_when_peer_already_exists(&[]);
}

#[test]
fn init_fails_when_peer_already_exists_with_gitlab_overrides() {
    assert_init_fails_when_peer_already_exists(&[
        "--provider",
        "openai",
        "--model",
        "model",
        "--repo",
        "group/subgroup/project",
        "--gitlab",
    ]);
}

#[test]
fn init_applies_provider_override() {
    assert_init_overrides_preserve_comments(Some("custom"), None, None, None);
}

#[test]
fn init_applies_model_override() {
    assert_init_overrides_preserve_comments(None, Some("namespace/new-model"), None, None);
}

#[test]
fn init_applies_repo_override_to_github_by_default() {
    assert_init_overrides_preserve_comments(None, None, Some("Org.Name/project_name-1"), None);
}

#[test]
fn init_applies_repo_override_to_explicit_github() {
    assert_init_overrides_preserve_comments(
        None,
        None,
        Some("Org.Name/project_name-1"),
        Some("--github"),
    );
}

#[test]
fn init_applies_repo_override_to_gitlab() {
    assert_init_overrides_preserve_comments(
        None,
        None,
        Some("group/subgroup/project"),
        Some("--gitlab"),
    );
}

#[test]
fn init_applies_all_overrides_with_default_github() {
    assert_init_overrides_preserve_comments(
        Some("openai"),
        Some("gpt-5.6-terra"),
        Some("owner/repository"),
        None,
    );
}

#[test]
fn init_stores_empty_overrides_without_falling_back_to_defaults() {
    assert_init_overrides(Some(""), Some(""), Some(""), None);
}

#[test]
fn init_serializes_quotes_and_backslashes_as_valid_toml() {
    let value = "quotes: \"'\" backslash: \\";
    assert_init_overrides(Some(value), Some(value), Some(value), None);
}

#[test]
fn init_rejects_github_and_gitlab_before_creating_peer() {
    let (mut cmd, tmp) = peer_in_tmp();
    git_init(tmp.path());
    let out = cmd
        .args(["init", "--github", "--gitlab", "--repo", "owner/repository"])
        .output()
        .unwrap();

    assert_eq!(out.status.code(), Some(2));
    assert!(!tmp.path().join(".peer").exists());
}
