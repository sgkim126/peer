//! Scripted glab subprocesses shared by command and orchestration tests.
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use serde_json::Value;
use url::Url;

use crate::glab::GlabClient;

pub struct Reply {
    pub status: u16,
    pub body: String,
    pub headers: Vec<String>,
    pub delay: Duration,
    pub disconnect: bool,
}

impl Reply {
    pub fn json(value: Value) -> Self {
        Self {
            status: 200,
            body: value.to_string(),
            headers: Vec::new(),
            delay: Duration::ZERO,
            disconnect: false,
        }
    }

    pub fn header(mut self, value: &str) -> Self {
        self.headers.push(value.to_string());
        self
    }

    pub fn status(mut self, status: u16) -> Self {
        self.status = status;
        self
    }

    #[expect(dead_code)]
    pub fn failure(status: u16, body: Value) -> Self {
        Self::json(body).status(status)
    }
}

pub struct Server {
    directory: tempfile::TempDir,
}

impl Server {
    pub async fn start(replies: Vec<Reply>) -> Self {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("cursor"), "0\n").unwrap();
        for (index, reply) in replies.iter().enumerate() {
            let path = directory.path().join(index.to_string());
            fs::create_dir(&path).unwrap();
            let headers = reply
                .headers
                .iter()
                .map(|header| {
                    format!(
                        "{}\r\n",
                        header.replace("{base}", "https://gitlab.com/api/v4/")
                    )
                })
                .collect::<String>();
            fs::write(
                path.join("response"),
                if reply.disconnect {
                    String::new()
                } else {
                    format!(
                        "HTTP/2.0 {} Test\r\nContent-Type: application/json\r\n{headers}\r\n{}",
                        reply.status, reply.body
                    )
                },
            )
            .unwrap();
            fs::write(
                path.join("exit"),
                if (200..300).contains(&reply.status) && !reply.disconnect {
                    "0\n"
                } else {
                    "1\n"
                },
            )
            .unwrap();
            fs::write(
                path.join("delay"),
                format!("{}\n", reply.delay.as_secs_f64()),
            )
            .unwrap();
        }
        let executable = directory.path().join("glab");
        fs::write(&executable, SCRIPT).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        Self { directory }
    }

    pub fn glab(&self, timeout: Duration) -> GlabClient {
        GlabClient::new(self.directory.path().join("glab"), timeout)
    }

    #[expect(dead_code)]
    pub fn invocation(&self, index: usize) -> (Vec<String>, Vec<u8>) {
        let path = self.directory.path().join(index.to_string());
        let args = fs::read_to_string(path.join("args"))
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        let stdin = fs::read(path.join("stdin")).unwrap_or_default();
        (args, stdin)
    }

    /// Normalize endpoint/body requests; command tests separately assert exact argv.
    pub fn requests(&self) -> Vec<String> {
        let count: usize = fs::read_to_string(self.directory.path().join("cursor"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        (0..count)
            .filter_map(|index| {
                let path = self.directory.path().join(index.to_string());
                let arguments = fs::read_to_string(path.join("args")).ok()?;
                let args: Vec<_> = arguments.lines().collect();
                assert_eq!(args[0], "api");
                let url = Url::parse(args[1]).unwrap_or_else(|_| {
                    Url::parse("https://gitlab.com/api/v4/")
                        .unwrap()
                        .join(args[1])
                        .unwrap()
                });
                let method = args
                    .windows(2)
                    .find(|pair| pair[0] == "--method")
                    .map(|pair| pair[1])
                    .unwrap_or("GET");
                let endpoint = format!(
                    "{}{}",
                    url.path(),
                    url.query()
                        .map(|query| format!("?{query}"))
                        .unwrap_or_default()
                );
                let headers = args
                    .windows(2)
                    .filter(|pair| pair[0] == "--header")
                    .map(|pair| format!("{}\r\n", pair[1]))
                    .collect::<String>();
                let stdin = fs::read_to_string(path.join("stdin")).unwrap_or_default();
                Some(format!(
                    "{method} {endpoint} HTTP/1.1\r\n{headers}\r\n{stdin}"
                ))
            })
            .collect()
    }

    #[expect(dead_code)]
    pub fn posts(&self) -> Vec<(String, Value)> {
        self.requests()
            .into_iter()
            .filter(|request| request.starts_with("POST "))
            .map(|request| {
                let (headers, body) = request.split_once("\r\n\r\n").unwrap();
                (
                    headers.lines().next().unwrap().to_string(),
                    serde_json::from_str(body).unwrap(),
                )
            })
            .collect()
    }
}

const SCRIPT: &str = r#"#!/bin/sh
set -eu
root=${0%/*}
read -r index < "$root/cursor"
next=$((index + 1))
printf '%s\n' "$next" > "$root/cursor"
reply="$root/$index"
if [ ! -d "$reply" ]; then
    echo 'unexpected glab invocation: no scripted response' >&2
    exit 1
fi
printf '%s\n' "$@" > "$reply/args"
for arg in "$@"; do
    if [ "$arg" = --input ]; then
        /bin/cat > "$reply/stdin"
        break
    fi
done
read -r code < "$reply/exit"
read -r delay < "$reply/delay"
if [ "$delay" != 0 ]; then
    /bin/sleep "$delay"
fi
/bin/cat "$reply/response"
if [ "$code" != 0 ]; then
    echo 'glab: scripted command failed' >&2
fi
exit "$code"
"#;
