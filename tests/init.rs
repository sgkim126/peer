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
