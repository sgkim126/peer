use std::fs;
use std::num::NonZeroU64;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use serde_json::Value;

use crate::gh::GhClient;
use crate::git::CommitHash;
use crate::github::Repository;

use super::*;

mod api;
mod create_conversation_comment;
mod create_review_comment;
mod list_commit_comments;
mod list_conversation_feedback;
mod list_pr_files;
mod list_review_feedback;

struct Reply {
    stdout: Vec<u8>,
    code: i32,
}

impl Reply {
    fn api(status: u16, headers: &[&str], body: &str) -> Self {
        let mut stdout = format!("HTTP/2.0 {status} Test\r\n");
        for header in headers {
            stdout.push_str(header);
            stdout.push_str("\r\n");
        }
        stdout.push_str("\r\n");
        stdout.push_str(body);
        Self {
            stdout: stdout.into_bytes(),
            code: i32::from(!(200..300).contains(&status)),
        }
    }

    fn json(value: Value) -> Self {
        Self::api(200, &[], &value.to_string())
    }

    fn plain(text: &str, code: i32) -> Self {
        Self {
            stdout: text.as_bytes().to_vec(),
            code,
        }
    }
}

struct FakeGh {
    dir: tempfile::TempDir,
    client: GhClient,
}

impl FakeGh {
    fn new(replies: Vec<Reply>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("gh");
        fs::write(
            &executable,
            r##"#!/bin/sh
task_dir=$(dirname "$0")
task_index=$(cat "$task_dir/index")
task_index=$((task_index + 1))
printf '%s' "$task_index" > "$task_dir/index"
printf '%s\n' "$@" > "$task_dir/args-$task_index"
cat > "$task_dir/stdin-$task_index"
cat "$task_dir/output-$task_index"
printf '%s' 'diagnostic from gh' >&2
exit "$(cat "$task_dir/status-$task_index")"
"##,
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(dir.path().join("index"), "0").unwrap();
        for (index, reply) in replies.into_iter().enumerate() {
            fs::write(
                dir.path().join(format!("output-{}", index + 1)),
                reply.stdout,
            )
            .unwrap();
            fs::write(
                dir.path().join(format!("status-{}", index + 1)),
                reply.code.to_string(),
            )
            .unwrap();
        }
        Self {
            dir,
            // These tests check command behavior, so allow for parallel process load.
            client: GhClient::new(executable, Duration::from_secs(30)),
        }
    }

    fn args(&self, index: usize) -> Vec<String> {
        fs::read_to_string(self.dir.path().join(format!("args-{index}")))
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn stdin(&self, index: usize) -> Vec<u8> {
        fs::read(self.dir.path().join(format!("stdin-{index}"))).unwrap()
    }

    fn requests(&self) -> usize {
        fs::read_to_string(self.dir.path().join("index"))
            .unwrap()
            .parse()
            .unwrap()
    }
}

fn repository() -> Repository {
    Repository::parse("owner/repo").unwrap()
}

fn number() -> NonZeroU64 {
    NonZeroU64::new(123).unwrap()
}

fn commit() -> CommitHash {
    serde_json::from_value(serde_json::json!("abc1234")).unwrap()
}

fn input(repository: &Repository) -> PullRequestInput<'_> {
    PullRequestInput {
        repository,
        number: number(),
    }
}
