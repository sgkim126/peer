use super::*;

use std::assert_matches;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

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

    fn status(mut self, status: u16) -> Self {
        self.status = status;
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
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = Url::parse(&format!(
            "http://{}/api/v4/",
            listener.local_addr().unwrap()
        ))
        .unwrap();
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
                    assert_ne!(count, 0, "request headers ended early");
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

    fn client(&self) -> GitLabClient {
        GitLabClient::new(
            "test-private-token",
            self.base.clone(),
            Duration::from_secs(2),
        )
        .unwrap()
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

#[tokio::test]
async fn rejects_credentials_on_the_api_origin_before_sending_a_request() {
    let server = Server::start(Vec::new()).await;
    let mut url = server.base.join("projects/5/notes").unwrap();
    url.set_username("user").unwrap();
    url.set_password(Some("password")).unwrap();
    assert_matches!(
        server.client().get::<Value>(url.as_str()).await,
        Err(GitLabError::InvalidPagination)
    );
    assert_eq!(server.requests(), Vec::<String>::new());
}

#[tokio::test]
async fn does_not_follow_redirect_responses() {
    let server = Server::start(vec![
        Reply::json(json!({}))
            .status(302)
            .header("Location: {base}redirect"),
        Reply::json(json!({})),
    ])
    .await;

    assert_matches!(
        server.client().get::<Value>("projects/5").await,
        Err(GitLabError::Api { status: 302, .. })
    );
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn does_not_retry_authentication_failures() {
    let server = Server::start(vec![
        Reply::json(json!({})).status(401),
        Reply::json(json!({})),
    ])
    .await;

    assert_matches!(
        server.client().get::<Value>("projects/5").await,
        Err(GitLabError::Api { status: 401, .. })
    );
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn does_not_retry_access_denied_responses() {
    let server = Server::start(vec![
        Reply::json(json!({})).status(403),
        Reply::json(json!({})),
    ])
    .await;

    assert_matches!(
        server.client().get::<Value>("projects/5").await,
        Err(GitLabError::Api { status: 403, .. })
    );
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn does_not_retry_not_found_responses() {
    let server = Server::start(vec![
        Reply::json(json!({})).status(404),
        Reply::json(json!({})),
    ])
    .await;

    assert_matches!(
        server.client().get::<Value>("projects/5").await,
        Err(GitLabError::Api { status: 404, .. })
    );
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn does_not_retry_rate_limit_responses() {
    let server = Server::start(vec![
        Reply::json(json!({})).status(429),
        Reply::json(json!({})),
    ])
    .await;

    assert_matches!(
        server.client().get::<Value>("projects/5").await,
        Err(GitLabError::Api { status: 429, .. })
    );
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn does_not_retry_server_errors() {
    let server = Server::start(vec![
        Reply::json(json!({})).status(500),
        Reply::json(json!({})),
    ])
    .await;

    assert_matches!(
        server.client().get::<Value>("projects/5").await,
        Err(GitLabError::Api { status: 500, .. })
    );
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn reports_too_many_requests_as_rate_limited() {
    let server = Server::start(vec![
        Reply::json(json!({})).status(429).header("Retry-After: 1"),
    ])
    .await;

    assert_matches!(
        server.client().get::<Value>("projects/5").await,
        Err(GitLabError::Api {
            status: 429,
            rate_limited: true,
            ..
        })
    );
}

#[tokio::test]
async fn reports_forbidden_with_exhausted_quota_as_rate_limited() {
    let server = Server::start(vec![
        Reply::json(json!({}))
            .status(403)
            .header("RateLimit-Remaining: 0"),
    ])
    .await;

    assert_matches!(
        server.client().get::<Value>("projects/5").await,
        Err(GitLabError::Api {
            status: 403,
            rate_limited: true,
            ..
        })
    );
}

#[tokio::test]
async fn reports_forbidden_with_remaining_quota_as_access_denied() {
    let server = Server::start(vec![
        Reply::json(json!({}))
            .status(403)
            .header("RateLimit-Remaining: 10"),
    ])
    .await;

    assert_matches!(
        server.client().get::<Value>("projects/5").await,
        Err(GitLabError::Api {
            status: 403,
            rate_limited: false,
            ..
        })
    );
}

#[tokio::test]
async fn reports_invalid_json_as_a_decode_error() {
    let mut reply = Reply::json(json!({}));
    reply.body = "invalid-json".into();
    let server = Server::start(vec![reply]).await;
    assert_matches!(
        server.client().get::<Value>("projects/5").await,
        Err(GitLabError::Decode { .. })
    );
}

#[tokio::test]
async fn reports_a_slow_response_as_a_request_timeout() {
    let mut reply = Reply::json(json!({}));
    reply.delay = Duration::from_millis(100);
    let server = Server::start(vec![reply]).await;
    let client = GitLabClient::new(
        "test-private-token",
        server.base.clone(),
        Duration::from_millis(10),
    )
    .unwrap();
    assert_matches!(client.get::<Value>("projects/5").await, Err(GitLabError::Request { source, .. }) if source.is_timeout());
}

#[test]
fn treats_an_empty_token_as_missing() {
    assert_matches!(private_token(""), Err(GitLabError::MissingToken));
}

#[test]
fn treats_a_space_only_token_as_missing() {
    assert_matches!(private_token(" "), Err(GitLabError::MissingToken));
}

#[test]
fn treats_control_whitespace_only_tokens_as_missing() {
    assert_matches!(private_token("\t\r\n"), Err(GitLabError::MissingToken));
}

#[test]
fn rejects_tokens_with_leading_spaces() {
    assert_matches!(private_token(" leading"), Err(GitLabError::InvalidToken));
}

#[test]
fn rejects_tokens_with_trailing_spaces() {
    assert_matches!(private_token("trailing "), Err(GitLabError::InvalidToken));
}

#[test]
fn rejects_tokens_with_embedded_spaces() {
    assert_matches!(private_token("with space"), Err(GitLabError::InvalidToken));
}

#[test]
fn rejects_tokens_with_embedded_newlines() {
    assert_matches!(
        private_token("secret\nheader"),
        Err(GitLabError::InvalidToken)
    );
}

#[test]
fn rejects_tokens_with_null_bytes() {
    assert_matches!(private_token("secret\0"), Err(GitLabError::InvalidToken));
}

#[test]
fn private_token_headers_are_sensitive() {
    let token = private_token("glpat-secret-value").unwrap();

    assert!(token.is_sensitive());
}

#[test]
fn debug_output_does_not_expose_private_tokens() {
    let token = private_token("glpat-secret-value").unwrap();

    assert!(!format!("{token:?}").contains("glpat-secret-value"));
}
