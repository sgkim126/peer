use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn cache_version() -> String {
    env!("CARGO_PKG_VERSION")
        .split('.')
        .take(2)
        .collect::<Vec<_>>()
        .join(".")
}

fn project() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let peer = directory.path().join(".peer");
    let cache = peer.join("cache");
    let nested = directory.path().join("nested");
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::create_dir(&nested).unwrap();
    std::fs::write(peer.join("config.toml"), "not valid toml").unwrap();
    (directory, cache, nested)
}

fn run_prune(cwd: &Path, all: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_peer"));
    command.arg("prune").current_dir(cwd);
    if all {
        command.arg("--all");
    }
    command.output().unwrap()
}

#[test]
fn prune_removes_only_outdated_cache_versions() {
    let (_directory, cache, nested) = project();
    let current = cache_version();
    for name in ["0.0", current.as_str(), "999999.0", "invalid"] {
        std::fs::create_dir(cache.join(name)).unwrap();
    }
    std::fs::write(cache.join("loose.tmp"), "temporary").unwrap();

    let output = run_prune(&nested, false);

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "pruned 1 old cache version directories\n"
    );
    assert!(!cache.join("0.0").exists());
    assert!(cache.join(current).is_dir());
    assert!(cache.join("999999.0").is_dir());
    assert!(cache.join("invalid").is_dir());
    assert!(cache.join("loose.tmp").is_file());
}

#[test]
fn prune_all_removes_every_cache_entry() {
    let (_directory, cache, nested) = project();
    std::fs::create_dir_all(cache.join(cache_version()).join("provider")).unwrap();
    std::fs::write(
        cache.join(cache_version()).join("provider/value.json"),
        "{}",
    )
    .unwrap();
    std::fs::write(cache.join("loose.tmp"), "temporary").unwrap();

    let output = run_prune(&nested, true);

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "pruned 2 cache entries\n"
    );
    assert!(cache.is_dir());
    assert_eq!(std::fs::read_dir(cache).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn prune_does_not_follow_a_symbolic_link_peer_directory() {
    use std::os::unix::fs::symlink;

    let project = tempfile::tempdir().unwrap();
    let linked = tempfile::tempdir().unwrap();
    let linked_peer = linked.path().join(".peer");
    let linked_cache = linked_peer.join("cache");
    std::fs::create_dir_all(&linked_cache).unwrap();
    std::fs::write(linked_peer.join("config.toml"), "not valid toml").unwrap();
    std::fs::write(linked_cache.join("keep"), "cached").unwrap();
    symlink(&linked_peer, project.path().join(".peer")).unwrap();

    let output = run_prune(project.path(), true);

    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("is a symbolic link")
    );
    assert!(linked_cache.join("keep").is_file());
}
