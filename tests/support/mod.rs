use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Give each child its own gh executable and authentication configuration.
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
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
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
