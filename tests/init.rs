use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

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
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not in a git repository"),
        "stderr: {stderr}",
    );
}

#[test]
fn init_preserves_git_error_in_bare_repo() {
    let (mut cmd, tmp) = peer_in_tmp();
    Command::new("git")
        .args(["init", "--bare"])
        .current_dir(tmp.path())
        .output()
        .unwrap()
        .assert_success();

    let out = cmd.env("LC_ALL", "C").arg("init").output().unwrap();

    assert!(!out.status.success());
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("cannot run git"), "stderr: {stderr}");
    assert!(
        stderr.contains("this operation must be run in a work tree"),
        "stderr: {stderr}"
    );
    assert!(
        !stderr.contains("not in a git repository"),
        "stderr: {stderr}"
    );
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn init_fails_when_git_is_unavailable() {
    let (mut cmd, _tmp) = peer_in_tmp();
    let out = cmd.env("PATH", "").arg("init").output().unwrap();

    assert!(!out.status.success());
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("cannot run git"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr),
    );
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

    let config_path = tmp.path().join(".peer").join("config.toml");
    assert!(config_path.exists());
    assert_eq!(
        std::fs::read_to_string(tmp.path().join(".peer").join(".gitignore")).unwrap(),
        "cache/\n"
    );
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn init_succeeds_from_symlinked_repo_root() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let repo_link = tmp.path().join("repo-link");
    std::fs::create_dir(&repo).unwrap();
    git_init(&repo);
    std::os::unix::fs::symlink(&repo, &repo_link).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_peer"));
    cmd.current_dir(&repo_link);

    let out = cmd.arg("init").output().unwrap();

    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(repo.join(".peer").join("config.toml").exists());
}

#[test]
fn init_from_git_subdirectory_points_to_repo_root_without_creating_peer_dir() {
    let (mut cmd, tmp) = peer_in_tmp();
    git_init(tmp.path());
    let subdirectory = tmp.path().join("src").join("nested");
    std::fs::create_dir_all(&subdirectory).unwrap();
    cmd.current_dir(&subdirectory);

    let out = cmd.arg("init").output().unwrap();

    assert!(!out.status.success());
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("peer init must be run from the repository root"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains(&tmp.path().display().to_string()),
        "stderr: {stderr}"
    );
    assert!(!tmp.path().join(".peer").exists());
    assert!(!subdirectory.join(".peer").exists());
}

#[test]
fn init_fails_when_peer_already_exists() {
    let (mut cmd, tmp) = peer_in_tmp();
    git_init(tmp.path());
    cmd.arg("init").output().unwrap().assert_success();

    let out = peer_in(&tmp).arg("init").output().unwrap();
    assert!(!out.status.success());
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn default_config_contains_provider_examples_and_pricing() {
    let (mut cmd, tmp) = peer_in_tmp();
    git_init(tmp.path());
    cmd.arg("init").output().unwrap().assert_success();

    let content = std::fs::read_to_string(tmp.path().join(".peer").join("config.toml")).unwrap();
    assert!(content.contains("mistral"), "mistral provider missing");
    assert!(content.contains("openai"), "openai provider missing");
    assert!(content.contains("anthropic"), "anthropic provider missing");
    assert!(content.contains("gemini"), "gemini provider missing");
    assert!(
        content.contains("mistral-large-2512"),
        "mistral-large-2512 model missing"
    );
    assert!(
        content.contains("input_per_1m_usd"),
        "input pricing missing"
    );
    assert!(
        content.contains("output_per_1m_usd"),
        "output pricing missing"
    );
}
