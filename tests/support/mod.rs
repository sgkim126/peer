use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Give each child its own gh executable and authentication configuration.
#[allow(dead_code)] // Each integration test binary uses the CLI helpers it needs.
pub fn configure_gh(command: &mut Command, directory: &Path, available: bool) -> PathBuf {
    let bin = directory.join(if available {
        "fake-gh-bin"
    } else {
        "empty-bin"
    });
    std::fs::create_dir_all(&bin).unwrap();
    let calls = directory.join("gh-calls");
    if available {
        let executable = bin.join("gh");
        std::fs::write(
            &executable,
            r#"#!/bin/sh
printf '%s\n' "$*" >> "$PEER_TEST_GH_CALLS"
if [ "$1" = '--version' ]; then
    printf 'gh version 2.101.0 (test)\n'
    exit 0
fi
printf '%s\n' 'gh: authentication required; run gh auth login' >&2
exit 4
"#,
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut paths = vec![bin];
    if available {
        let current_path = command
            .get_envs()
            .find(|(key, _)| *key == "PATH")
            .map(|(_, value)| value.map(std::ffi::OsStr::to_owned))
            .unwrap_or_else(|| std::env::var_os("PATH"));
        if let Some(current_path) = current_path {
            paths.extend(std::env::split_paths(&current_path));
        }
    }
    command
        .env("PATH", std::env::join_paths(paths).unwrap())
        .env("GH_CONFIG_DIR", directory.join("gh-config"))
        .env("PEER_TEST_GH_CALLS", &calls);
    for variable in [
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "GH_ENTERPRISE_TOKEN",
        "GITHUB_ENTERPRISE_TOKEN",
        "GH_HOST",
        "GH_DEBUG",
        "GH_REPO",
    ] {
        command.env_remove(variable);
    }
    calls
}

/// Give each child its own glab executable and authentication configuration.
#[allow(dead_code)] // Each integration test binary uses the CLI helpers it needs.
pub fn configure_glab(command: &mut Command, directory: &Path, available: bool) -> PathBuf {
    let bin = directory.join(if available {
        "fake-glab-bin"
    } else {
        "empty-bin"
    });
    std::fs::create_dir_all(&bin).unwrap();
    let calls = directory.join("glab-calls");
    if available {
        let executable = bin.join("glab");
        std::fs::write(
            &executable,
            r#"#!/bin/sh
printf '%s\n' "$*" >> "$PEER_TEST_GLAB_CALLS"
if [ "$1" = '--version' ]; then
    printf 'glab version 1.93.0 (test)\n'
    exit 0
fi
printf '%s\n' 'glab: authentication required; run glab auth login' >&2
exit 4
"#,
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut paths = vec![bin];
    if available {
        let current_path = command
            .get_envs()
            .find(|(key, _)| *key == "PATH")
            .map(|(_, value)| value.map(std::ffi::OsStr::to_owned))
            .unwrap_or_else(|| std::env::var_os("PATH"));
        if let Some(current_path) = current_path {
            paths.extend(std::env::split_paths(&current_path));
        }
    }
    command
        .env("PATH", std::env::join_paths(paths).unwrap())
        .env("GLAB_CONFIG_DIR", directory.join("glab-config"))
        .env("PEER_TEST_GLAB_CALLS", &calls);
    for variable in [
        "GITLAB_TOKEN",
        "GITLAB_ACCESS_TOKEN",
        "OAUTH_TOKEN",
        "GITLAB_HOST",
        "GITLAB_URI",
        "GITLAB_REPO",
        "GLAB_DEBUG",
        "DEBUG",
    ] {
        command.env_remove(variable);
    }
    calls
}
