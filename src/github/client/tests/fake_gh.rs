//! Scripted gh process responses for the GitHub orchestration scenarios.
use std::fs;
use std::os::unix::fs::PermissionsExt;

use url::Url;

use super::{Duration, GitHubClient, Reply, Value, json};
use crate::gh::GhClient;

pub struct Server {
    directory: tempfile::TempDir,
}

impl Server {
    pub async fn start(replies: Vec<Reply>) -> Self {
        Self::start_with(|_| replies).await
    }

    pub async fn start_with(replies: impl FnOnce(&Url) -> Vec<Reply>) -> Self {
        let base = Url::parse("https://api.github.com/").unwrap();
        let replies = replies(&base);
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("cursor"), "0\n").unwrap();
        for (index, reply) in replies.iter().enumerate() {
            let path = directory.path().join(index.to_string());
            fs::create_dir(&path).unwrap();
            let headers = reply
                .headers
                .iter()
                .map(|header| format!("{}\r\n", header.replace("{base}", base.as_str())))
                .collect::<String>();
            fs::write(
                path.join("api"),
                format!(
                    "HTTP/2.0 {} Test\r\nContent-Type: application/json\r\n{headers}\r\n{}",
                    reply.status, reply.body
                ),
            )
            .unwrap();
            let exit = if (200..300).contains(&reply.status) {
                "0\n"
            } else {
                "1\n"
            };
            fs::write(path.join("exit"), exit).unwrap();
            fs::write(
                path.join("delay"),
                format!("{}\n", reply.delay.as_secs_f64()),
            )
            .unwrap();
            let parsed = serde_json::from_str::<Value>(&reply.body).ok();
            fs::write(
                path.join("comment"),
                parsed
                    .as_ref()
                    .and_then(|value| value["html_url"].as_str())
                    .unwrap_or_default(),
            )
            .unwrap();

            let (comments, view_exit) = match serde_json::from_str::<Vec<Value>>(&reply.body) {
                Ok(comments) => (
                    comments
                        .into_iter()
                        .map(|mut comment| {
                            if let Some(object) = comment.as_object_mut()
                                && let Some(url) = object.remove("html_url")
                            {
                                object.insert("url".into(), url);
                            }
                            comment
                        })
                        .collect::<Vec<_>>(),
                    exit,
                ),
                Err(_) => (Vec::new(), "1\n"),
            };
            fs::write(path.join("view"), json!({"comments": comments}).to_string()).unwrap();
            fs::write(path.join("view-exit"), view_exit).unwrap();
        }
        let executable = directory.path().join("gh");
        fs::write(&executable, SCRIPT).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        Self { directory }
    }

    pub fn client(&self) -> GitHubClient {
        // Leave room for process startup under parallel test load. Timeout tests
        // set their own deadline through client_with_timeout.
        self.client_with_timeout(Duration::from_secs(30))
    }

    pub fn client_with_timeout(&self, timeout: Duration) -> GitHubClient {
        GitHubClient::new(GhClient::new(self.directory.path().join("gh"), timeout))
    }

    /// Normalize recorded operations so domain tests can retain their endpoint/body assertions.
    /// Exact argv and stdin contracts are tested by the typed command layer.
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
                let field = |name| {
                    args.windows(2)
                        .find(|pair| pair[0] == name)
                        .map(|pair| pair[1])
                };
                let stdin = fs::read_to_string(path.join("stdin")).unwrap_or_default();
                let (method, endpoint, body) = if args[0] == "api" {
                    let url = Url::parse(args[1]).unwrap_or_else(|_| {
                        Url::parse("https://api.github.com/")
                            .unwrap()
                            .join(args[1])
                            .unwrap()
                    });
                    let endpoint = format!(
                        "{}{}",
                        url.path(),
                        url.query()
                            .map(|query| format!("?{query}"))
                            .unwrap_or_default()
                    );
                    (field("--method").unwrap_or("GET"), endpoint, stdin)
                } else {
                    assert_eq!(args[0], "pr");
                    let repo = field("--repo")
                        .unwrap()
                        .strip_prefix("github.com/")
                        .unwrap();
                    let endpoint = format!("/repos/{repo}/issues/{}/comments", args[2]);
                    if args[1] == "view" {
                        ("GET", format!("{endpoint}?per_page=100"), String::new())
                    } else {
                        assert_eq!(args[1], "comment");
                        ("POST", endpoint, json!({"body":stdin}).to_string())
                    }
                };
                let headers = args
                    .windows(2)
                    .filter(|pair| pair[0] == "--header")
                    .map(|pair| format!("{}\r\n", pair[1]))
                    .collect::<String>();
                Some(format!(
                    "{method} {endpoint} HTTP/1.1\r\n{headers}\r\n{body}"
                ))
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
    echo 'unexpected gh invocation: no scripted response' >&2
    exit 1
fi
printf '%s\n' "$@" > "$reply/args"
response=api
if [ "$1" = pr ]; then
    response=$2
fi
read -r code < "$reply/exit"
if [ "$response" = view ]; then
    read -r code < "$reply/view-exit"
fi
for arg in "$@"; do
    if [ "$arg" = --input ] || [ "$arg" = --body-file ]; then
        /bin/cat > "$reply/stdin"
        break
    fi
done
read -r delay < "$reply/delay"
if [ "$delay" != 0 ]; then
    /bin/sleep "$delay"
fi
if [ "$response" = api ] || [ "$code" = 0 ]; then
    /bin/cat "$reply/$response"
fi
if [ "$code" != 0 ]; then
    echo 'gh: scripted command failed' >&2
fi
exit "$code"
"#;
