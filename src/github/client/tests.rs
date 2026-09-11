use super::*;

use std::assert_matches;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

mod publishing;

struct Reply {
    status: u16,
    body: String,
    headers: Vec<String>,
    delay: Duration,
}

impl Reply {
    fn json(value: Value) -> Self {
        Self {
            status: 200,
            body: value.to_string(),
            headers: Vec::new(),
            delay: Duration::ZERO,
        }
    }

    fn header(mut self, value: &str) -> Self {
        self.headers.push(value.to_string());
        self
    }
}

struct Server {
    base: Url,
    requests: Arc<Mutex<Vec<String>>>,
    task: JoinHandle<()>,
}

impl Server {
    async fn start(replies: Vec<Reply>) -> Self {
        Self::start_with(|_| replies).await
    }

    async fn start_with(replies: impl FnOnce(&Url) -> Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let replies = replies(&base);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let received = requests.clone();
        let address = base.to_string();
        let task = tokio::spawn(async move {
            for reply in replies {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 1024];
                while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                    let count = stream.read(&mut buffer).await.unwrap();
                    if count == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..count]);
                }
                let header_end = request
                    .windows(4)
                    .position(|bytes| bytes == b"\r\n\r\n")
                    .unwrap()
                    + 4;
                let content_length = String::from_utf8_lossy(&request[..header_end])
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                    .unwrap_or(0);
                while request.len() < header_end + content_length {
                    let count = stream.read(&mut buffer).await.unwrap();
                    assert_ne!(count, 0, "request body ended early");
                    request.extend_from_slice(&buffer[..count]);
                }
                received
                    .lock()
                    .unwrap()
                    .push(String::from_utf8(request).unwrap());
                tokio::time::sleep(reply.delay).await;
                let extra = reply
                    .headers
                    .iter()
                    .map(|header| format!("{}\r\n", header.replace("{base}", &address)))
                    .collect::<String>();
                let response = format!(
                    "HTTP/1.1 {} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n{}",
                    reply.status,
                    reply.body.len(),
                    extra,
                    reply.body
                );
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });
        Self {
            base,
            requests,
            task,
        }
    }

    fn client(&self) -> GitHubClient {
        GitHubClient::new("test-token", self.base.clone(), Duration::from_secs(2)).unwrap()
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn repository() -> Repository {
    Repository::parse("owner/repo").unwrap()
}
fn number() -> NonZeroU64 {
    NonZeroU64::new(123).unwrap()
}
fn pull() -> Reply {
    Reply::json(json!({
        "title": "Title", "body": "Description",
        "base": {"sha": "0123456"}, "head": {"sha": "abc1234"}, "commits": 1,
    }))
}

fn comment(id: u64, author: Value) -> Value {
    json!({"id": id, "created_at": "2026-01-01T00:00:00Z", "user": author, "body": format!("Comment {id}")})
}

#[tokio::test]
async fn paginates_pull_request_commits_in_api_order() {
    let pull = json!({
        "title": "Title", "body": "Description",
        "base": {"sha": "0123456"}, "head": {"sha": "def5678"}, "commits": 2,
    });
    let server = Server::start(vec![
        Reply::json(pull.clone()),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([{"sha": "abc1234"}])).header(
            "Link: <{base}repos/owner/repo/pulls/123/commits?per_page=100&page=2>; rel=\"next\"",
        ),
        Reply::json(json!([{"sha": "def5678"}])),
        Reply::json(pull),
    ])
    .await;
    let input = server
        .client()
        .review_input(&repository(), number())
        .await
        .unwrap();
    assert_eq!(input.context.title.as_deref(), Some("Title"));
    assert_eq!(
        input
            .commits
            .iter()
            .map(AsRef::as_ref)
            .collect::<Vec<&str>>(),
        ["abc1234", "def5678"]
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 7);
    assert!(requests[4].starts_with("GET /repos/owner/repo/pulls/123/commits?per_page=100 "));
    assert!(
        requests[5].starts_with("GET /repos/owner/repo/pulls/123/commits?per_page=100&page=2 ")
    );
    assert!(requests[6].starts_with("GET /repos/owner/repo/pulls/123 "));
}

#[tokio::test]
async fn rejects_changed_base_head_or_count_after_commit_pagination() {
    for (base, head, count) in [
        ("7654321", "def5678", 2),
        ("0123456", "abc1234", 2),
        ("0123456", "def5678", 3),
    ] {
        let server = Server::start(vec![
            Reply::json(json!({
                "title": "Title", "body": "Description",
                "base": {"sha": "0123456"}, "head": {"sha": "def5678"}, "commits": 2,
            })),
            Reply::json(json!([])),
            Reply::json(json!([])),
            Reply::json(json!([])),
            Reply::json(json!([{"sha": "abc1234"}])).header(
                "Link: <{base}repos/owner/repo/pulls/123/commits?per_page=100&page=2>; rel=\"next\"",
            ),
            Reply::json(json!([{"sha": "def5678"}])),
            Reply::json(json!({
                "title": "Title", "body": "Description",
                "base": {"sha": base}, "head": {"sha": head}, "commits": count,
            })),
        ])
        .await;
        assert_matches!(
            server.client().review_input(&repository(), number()).await,
            Err(GitHubError::IncompleteCommits)
        );
        let requests = server.requests();
        assert_eq!(requests.len(), 7);
        assert!(requests[6].starts_with("GET /repos/owner/repo/pulls/123 "));
    }
}

#[tokio::test]
async fn pull_request_revalidation_failure_discards_the_whole_input() {
    let mut failure = Reply::json(json!({}));
    failure.status = 500;
    let server = Server::start(vec![
        pull(),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([{"sha": "abc1234"}])),
        failure,
    ])
    .await;
    assert_matches!(
        server.client().review_input(&repository(), number()).await,
        Err(GitHubError::Api { status: 500, .. })
    );
    assert_eq!(server.requests().len(), 6);
}

#[tokio::test]
async fn rejects_incomplete_or_changed_pull_request_commits() {
    for (count, commits) in [
        (1, json!([])),
        (0, json!([])),
        (2, json!([{"sha": "abc1234"}])),
        (1, json!([{"sha": "def5678"}])),
        (1, json!([{"sha": "def5678"}, {"sha": "abc1234"}])),
        (251, json!(vec![json!({"sha": "abc1234"}); 250])),
    ] {
        let server = Server::start(vec![
            Reply::json(json!({
                "title": "Title", "body": null,
                "base": {"sha": "0123456"}, "head": {"sha": "abc1234"}, "commits": count,
            })),
            Reply::json(json!([])),
            Reply::json(json!([])),
            Reply::json(json!([])),
            Reply::json(commits),
        ])
        .await;
        assert_matches!(
            server.client().review_input(&repository(), number()).await,
            Err(GitHubError::IncompleteCommits)
        );
    }
}

#[tokio::test]
async fn rejects_invalid_commit_hashes_from_the_commits_endpoint() {
    let server = Server::start(vec![
        pull(),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([{"sha": "not-a-commit"}])),
    ])
    .await;
    assert_matches!(
        server.client().review_input(&repository(), number()).await,
        Err(GitHubError::Decode { endpoint, .. }) if endpoint.ends_with("/commits")
    );
}

#[tokio::test]
async fn a_later_commit_page_failure_discards_the_whole_input() {
    let mut failure = Reply::json(json!({}));
    failure.status = 500;
    let server = Server::start(vec![
        pull(),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([{"sha": "abc1234"}]))
            .header("Link: <{base}repos/owner/repo/pulls/123/commits?page=2>; rel=\"next\""),
        failure,
    ])
    .await;
    assert_matches!(
        server.client().review_input(&repository(), number()).await,
        Err(GitHubError::Api { status: 500, .. })
    );
    assert_eq!(server.requests().len(), 6);
}

#[tokio::test]
async fn paginates_comments_and_matches_direct_input() {
    let server = Server::start(vec![
        pull(),
        Reply::json(json!([comment(2, Value::Null)])).header(
            "Link: <{base}repos/owner/repo/issues/123/comments?per_page=100&page=2>; rel=\"next\"",
        ),
        Reply::json(json!([comment(
            1,
            json!({"login": "bot[bot]", "type": "Bot"})
        )])),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([{"sha": "abc1234"}])),
        pull(),
    ])
    .await;
    let context = server
        .client()
        .review_input(&repository(), number())
        .await
        .unwrap()
        .context;
    let directory = tempfile::tempdir().unwrap();
    let body = directory.path().join("body.md");
    let comments = directory.path().join("comments.json");
    std::fs::write(&body, "Description").unwrap();
    std::fs::write(&comments, r#"[{"comments":[{"author":"bot[bot]","body":"Comment 1"}]},{"comments":[{"author":"unknown","body":"Comment 2"}]}]"#).unwrap();
    assert_eq!(
        context,
        ReviewContext::load(Some("Title".into()), Some(&body), Some(&comments)).unwrap()
    );

    let requests = server.requests();
    assert_eq!(requests.len(), 7);
    assert!(requests[0].starts_with("GET /repos/owner/repo/pulls/123 "));
    assert!(requests[1].starts_with("GET /repos/owner/repo/issues/123/comments?per_page=100 "));
    assert!(requests[2].contains("per_page=100&page=2"));
    assert!(requests[3].starts_with("GET /repos/owner/repo/pulls/123/reviews?per_page=100 "));
    assert!(requests[4].starts_with("GET /repos/owner/repo/pulls/123/comments?per_page=100 "));
    assert!(requests[5].starts_with("GET /repos/owner/repo/pulls/123/commits?per_page=100 "));
    assert!(requests[6].starts_with("GET /repos/owner/repo/pulls/123 "));
    for request in requests {
        let request = request.to_ascii_lowercase();
        assert!(request.contains("authorization: bearer test-token\r\n"));
        assert!(request.contains("accept: application/vnd.github+json\r\n"));
        assert!(request.contains(&format!("x-github-api-version: {API_VERSION}\r\n")));
        assert!(request.contains(concat!("user-agent: peer/", env!("CARGO_PKG_VERSION"))));
    }
}

#[tokio::test]
async fn missing_body_and_comments_match_empty_input_files() {
    let pull = json!({
        "title": "Title", "body": null,
        "base": {"sha": "0123456"}, "head": {"sha": "abc1234"}, "commits": 1,
    });
    let server = Server::start(vec![
        Reply::json(pull.clone()),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([{"sha": "abc1234"}])),
        Reply::json(pull),
    ])
    .await;
    let context = server
        .client()
        .review_input(&repository(), number())
        .await
        .unwrap()
        .context;
    assert_eq!(context.body.as_deref(), Some(""));
    assert_eq!(context.comments, vec![]);
}

#[tokio::test]
async fn reports_http_failures_without_retrying_or_loading_comments() {
    for status in [401, 403, 404, 429, 500] {
        let mut reply = Reply::json(json!({"message": "error"}));
        reply.status = status;
        let server = Server::start(vec![reply]).await;
        let error = server.client().review_input(&repository(), number()).await;
        assert_matches!(error, Err(GitHubError::Api { status: actual, .. }) if actual == status);
        let error = error.unwrap_err();
        assert!(error.to_string().contains(&format!("HTTP {status}")));
        assert!(!format!("{error:?}").contains("test-token"));
        assert_eq!(server.requests().len(), 1);
    }
}

#[tokio::test]
async fn distinguishes_rate_limits_from_other_forbidden_responses() {
    let mut reply = Reply::json(json!({})).header("X-RateLimit-Remaining: 0");
    reply.status = 403;
    let server = Server::start(vec![reply]).await;
    let error = server.client().review_input(&repository(), number()).await;
    assert_matches!(
        error,
        Err(GitHubError::Api {
            rate_limited: true,
            ..
        })
    );
}

#[tokio::test]
async fn a_later_page_failure_discards_the_whole_context() {
    let mut failure = Reply::json(json!({}));
    failure.status = 500;
    let server = Server::start(vec![
        pull(),
        Reply::json(json!([comment(1, Value::Null)])).header("Link: <{base}next>; rel=\"next\""),
        failure,
    ])
    .await;
    assert_matches!(
        server.client().review_input(&repository(), number()).await,
        Err(GitHubError::Api { status: 500, .. })
    );
    assert_eq!(server.requests().len(), 3);
}

#[tokio::test]
async fn rejects_malformed_json_responses() {
    let mut reply = pull();
    reply.body = "not json".into();
    let server = Server::start(vec![reply]).await;
    assert_matches!(
        server.client().review_input(&repository(), number()).await,
        Err(GitHubError::Decode { .. })
    );
}

#[tokio::test]
async fn rejects_responses_without_title() {
    let mut reply = pull();
    reply.body = r#"{"body":"missing title"}"#.into();
    let server = Server::start(vec![reply]).await;
    assert_matches!(
        server.client().review_input(&repository(), number()).await,
        Err(GitHubError::Decode { .. })
    );
}

#[tokio::test]
async fn times_out_without_returning_partial_context() {
    let mut reply = pull();
    reply.delay = Duration::from_secs(1);
    let server = Server::start(vec![reply]).await;
    let client =
        GitHubClient::new("test-token", server.base.clone(), Duration::from_millis(50)).unwrap();
    let error = client.review_input(&repository(), number()).await;
    assert_matches!(error, Err(GitHubError::Request { ref source, .. }) if source.is_timeout());
}

#[tokio::test]
async fn rejects_pagination_to_another_origin() {
    let server = Server::start(vec![
        pull(),
        Reply::json(json!([])).header("Link: <https://example.com/next>; rel=\"next\""),
    ])
    .await;
    assert_matches!(
        server.client().review_input(&repository(), number()).await,
        Err(GitHubError::InvalidPagination)
    );
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn rejects_pagination_with_url_credentials() {
    let server = Server::start_with(|base| {
        let mut next = base.join("next").unwrap();
        next.set_username("user").unwrap();
        next.set_password(Some("password")).unwrap();
        vec![
            pull(),
            Reply::json(json!([])).header(&format!("Link: <{next}>; rel=\"next\"")),
        ]
    })
    .await;
    assert_matches!(
        server.client().review_input(&repository(), number()).await,
        Err(GitHubError::InvalidPagination)
    );
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn rejects_malformed_pagination_links() {
    let server = Server::start(vec![
        pull(),
        Reply::json(json!([])).header("Link: invalid; rel=\"next\""),
    ])
    .await;
    assert_matches!(
        server.client().review_input(&repository(), number()).await,
        Err(GitHubError::InvalidPagination)
    );
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn rejects_cyclic_pagination() {
    let server = Server::start(vec![
        pull(),
        Reply::json(json!([])).header(
            "Link: <{base}repos/owner/repo/issues/123/comments?per_page=100>; rel=\"next\"",
        ),
    ])
    .await;
    assert_matches!(
        server.client().review_input(&repository(), number()).await,
        Err(GitHubError::InvalidPagination)
    );
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn does_not_follow_redirects() {
    let mut reply = Reply::json(json!({})).header("Location: https://example.com/next");
    reply.status = 302;
    let server = Server::start(vec![reply]).await;
    assert_matches!(
        server.client().review_input(&repository(), number()).await,
        Err(GitHubError::Api { status: 302, .. })
    );
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn validates_and_redacts_tokens() {
    for token in ["", " \n\t"] {
        assert_matches!(authorization(token), Err(GitHubError::MissingToken));
    }
    for token in ["token\nsecret", " token", "token ", "한글"] {
        let error = authorization(token);
        assert_matches!(error, Err(GitHubError::InvalidToken));
        let error = error.unwrap_err();
        assert!(!format!("{error:?}").contains(token));
    }
    let header = authorization("test-secret").unwrap();
    assert!(header.is_sensitive());
    assert!(!format!("{header:?}").contains("test-secret"));
}

#[tokio::test]
async fn paginates_reviews_and_groups_inline_replies_across_pages() {
    let review = |id| {
        json!({"id": id, "state": "COMMENTED", "body": format!("Review {id}"),
        "submitted_at": "2026-01-01T00:00:00Z", "commit_id": "abc1234", "user": null})
    };
    let inline = |id, parent| {
        json!({"id": id, "in_reply_to_id": parent,
        "created_at": "2026-01-01T00:00:00Z", "body": format!("Inline {id}"), "user": {"login": "bot[bot]", "type": "Bot"},
        "path": "src/main.rs", "commit_id": "abc1234", "line": 42, "side": "RIGHT"})
    };
    let server = Server::start(vec![
        pull(),
        Reply::json(json!([])),
        Reply::json(json!([review(2)]))
            .header("Link: <{base}repos/owner/repo/pulls/123/reviews?page=2>; rel=\"next\""),
        Reply::json(json!([review(1)])),
        Reply::json(json!([inline(11, Some(10))]))
            .header("Link: <{base}repos/owner/repo/pulls/123/comments?page=2>; rel=\"next\""),
        Reply::json(json!([inline(10, None)])),
        Reply::json(json!([{"sha": "abc1234"}])),
        pull(),
    ])
    .await;
    let context = server
        .client()
        .review_input(&repository(), number())
        .await
        .unwrap()
        .context;
    assert_eq!(context.comments.len(), 3);
    assert_eq!(context.comments[0].comments[0].body, "Review 1");
    assert_eq!(context.comments[1].comments[0].body, "Review 2");
    assert_eq!(
        context.comments[2]
            .comments
            .iter()
            .map(|comment| comment.body.as_str())
            .collect::<Vec<_>>(),
        ["Inline 10", "Inline 11"]
    );
    assert_eq!(server.requests().len(), 8);
}

#[tokio::test]
async fn reviews_endpoint_failure_discards_the_whole_context() {
    let mut failure = Reply::json(json!({}));
    failure.status = 403;
    let server = Server::start(vec![
        pull(),
        Reply::json(json!([comment(1, Value::Null)])),
        failure,
    ])
    .await;
    assert_matches!(
        server.client().review_input(&repository(), number()).await,
        Err(GitHubError::Api { status: 403, .. })
    );
    assert_eq!(server.requests().len(), 3);
}

#[tokio::test]
async fn review_comments_endpoint_failure_discards_the_whole_context() {
    let mut failure = Reply::json(json!({}));
    failure.status = 403;
    let server = Server::start(vec![
        pull(),
        Reply::json(json!([comment(1, Value::Null)])),
        Reply::json(json!([])),
        failure,
    ])
    .await;
    assert_matches!(
        server.client().review_input(&repository(), number()).await,
        Err(GitHubError::Api { status: 403, .. })
    );
    assert_eq!(server.requests().len(), 4);
}
