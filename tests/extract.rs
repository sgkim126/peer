use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;
use tempfile::TempDir;

struct Repo {
    _tmp: TempDir,
    path: PathBuf,
}

impl Repo {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().to_path_buf();
        git(&path, &["init"]);
        git(&path, &["config", "user.email", "test@example.com"]);
        git(&path, &["config", "user.name", "Test"]);
        Command::new(env!("CARGO_BIN_EXE_peer"))
            .arg("init")
            .current_dir(&path)
            .output()
            .unwrap()
            .assert_success();
        Self { _tmp: tmp, path }
    }

    fn commit(&self, files: &[(&str, &[u8])], message: &str) -> String {
        for (name, content) in files {
            std::fs::write(self.path.join(name), content).unwrap();
            git(&self.path, &["add", name]);
        }
        git(&self.path, &["commit", "--no-gpg-sign", "-m", message]);
        head_hash(&self.path)
    }

    fn extract(&self, args: &[&str]) -> serde_json::Value {
        let out = self.extract_raw(args);
        out.assert_success();
        serde_json::from_slice(&out.stdout).unwrap()
    }

    fn extract_raw(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_peer"))
            .arg("extract")
            .args(args)
            .current_dir(&self.path)
            .output()
            .unwrap()
    }
}

fn git(path: &Path, args: &[&str]) {
    Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .unwrap()
        .assert_success();
}

fn head_hash(path: &Path) -> String {
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(path)
        .output()
        .unwrap();
    out.assert_success();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
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
fn commit_message_returns_message() {
    let repo = Repo::new();
    let hash = repo.commit(&[("f.txt", b"content")], "my commit message");

    assert_eq!(
        repo.extract(&["commit-message", &hash]),
        json!({
            "command": "commit-message",
            "hash": hash,
            "message": "my commit message"
        }),
    );
}

#[test]
fn commit_diff_contains_added_content() {
    let repo = Repo::new();
    let hash = repo.commit(&[("f.txt", b"hello world\n")], "add file");

    let json = repo.extract(&["commit-diff", &hash]);
    assert!(json["diff"].as_str().unwrap().contains("+hello world"));
}

#[test]
fn commit_files_shows_added_status() {
    let repo = Repo::new();
    let hash = repo.commit(&[("f.txt", b"content")], "add");

    assert_eq!(
        repo.extract(&["commit-files", &hash]),
        json!({
            "command": "commit-files",
            "hash": hash,
            "files": [{
                "path": "f.txt",
                "status": "added",
                "is_binary": false
            }]
        })
    );
}

#[test]
fn commit_files_rename_includes_source_path() {
    let repo = Repo::new();
    repo.commit(&[("old.txt", b"content here")], "initial");

    std::fs::rename(repo.path.join("old.txt"), repo.path.join("new.txt")).unwrap();
    git(&repo.path, &["add", "-u", "old.txt"]);
    git(&repo.path, &["add", "new.txt"]);
    git(&repo.path, &["commit", "--no-gpg-sign", "-m", "rename"]);
    let hash = head_hash(&repo.path);

    assert_eq!(
        repo.extract(&["commit-files", &hash]),
        json!({
            "command": "commit-files",
            "hash": hash,
            "files": [{
                "path": "new.txt",
                "status": "renamed",
                "source_path": "old.txt",
                "is_binary": false
            }]
        })
    );
}

#[test]
fn commit_files_detects_binary() {
    let repo = Repo::new();
    let hash = repo.commit(
        &[("data.bin", &[0x00u8, 0x01, 0x02, 0x03] as &[u8])],
        "add binary",
    );

    assert_eq!(
        repo.extract(&["commit-files", &hash]),
        json!({
            "command": "commit-files",
            "hash": hash,
            "files": [{
                "path": "data.bin",
                "status": "added",
                "is_binary": true
            }]
        })
    );
}

#[test]
fn commit_list_returns_oldest_to_newest() {
    let repo = Repo::new();
    let hash1 = repo.commit(&[("a.txt", b"a")], "first");
    let hash2 = repo.commit(&[("b.txt", b"b")], "second");
    let hash3 = repo.commit(&[("c.txt", b"c")], "third");

    let range = format!("{hash1}..HEAD");
    assert_eq!(
        repo.extract(&["commit-list", &range]),
        json!({
            "command": "commit-list",
            "range": range,
            "commits": [hash2, hash3]
        })
    );
}

#[test]
fn commit_list_rejects_three_dot_range() {
    let repo = Repo::new();
    repo.commit(&[("a.txt", b"a")], "commit");

    let range = "main...develop";
    let out = repo.extract_raw(&["commit-list", range]);
    assert!(!out.status.success());
    assert_eq!(out.status.code(), Some(1));

    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains(format!("{range} is not a two-dot range").as_str()));
}

#[test]
fn file_content_returns_text() {
    let repo = Repo::new();
    let hash = repo.commit(&[("hello.txt", b"hello world")], "add");

    assert_eq!(
        repo.extract(&["file-content", "--path", "hello.txt", &hash]),
        json!({
            "command": "file-content",
            "type": "text",
            "path": "hello.txt",
            "hash": hash,
            "content": "hello world"
        })
    );
}

#[test]
fn file_content_returns_binary() {
    let repo = Repo::new();
    let hash = repo.commit(
        &[("data.bin", &[0x00u8, 0x01, 0x02, 0x03] as &[u8])],
        "add binary",
    );

    assert_eq!(
        repo.extract(&["file-content", "--path", "data.bin", &hash]),
        json!({
            "command": "file-content",
            "type": "binary",
            "path": "data.bin",
            "hash": hash,
            "size": 4
        })
    );
}
