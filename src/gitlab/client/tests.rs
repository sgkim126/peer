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

fn repository() -> Repository {
    Repository::parse("group/subgroup/project").unwrap()
}

fn number() -> NonZeroU64 {
    NonZeroU64::new(123).unwrap()
}

fn merge_request() -> Value {
    json!({
        "id": 9876, "iid": 123, "project_id": 5, "target_project_id": 5,
        "source_project_id": 9, "source_branch": "feature", "target_branch": "main",
        "title": "Title", "description": "Description", "sha": "abc1234",
        "diff_refs": {"base_sha": "0123456", "head_sha": "abc1234", "start_sha": "9876543"}
    })
}

fn commits() -> Reply {
    Reply::json(json!([{"id": "abc1234"}]))
}

fn discussions() -> Reply {
    Reply::json(json!([]))
}

fn note(id: u64) -> Value {
    json!({"id": id, "body": format!("Comment {id}"), "author": {"username": "alice"},
        "created_at": "2026-01-01T00:00:00Z", "system": false})
}

#[tokio::test]
async fn loads_a_fork_merge_request_from_the_target_project() {
    let server = Server::start(vec![
        Reply::json(merge_request()),
        commits(),
        discussions(),
        discussions(),
        discussions(),
        Reply::json(merge_request()),
    ])
    .await;
    let input = server
        .client()
        .review_input(&repository(), number())
        .await
        .unwrap();
    assert_eq!(input.context.title.as_deref(), Some("Title"));
    assert_eq!(input.context.body.as_deref(), Some("Description"));
    assert_eq!(input.source.project_id, 5);
    assert_eq!(input.source.source_project_id, Some(9));
    assert_eq!(input.source.iid, 123);
    assert_eq!(input.commits, [CommitHash::new("abc1234").unwrap()]);
    let requests = server.requests();
    assert_eq!(requests.len(), 6);
    for index in [0, 1, 2, 5] {
        assert!(
            requests[index]
                .starts_with("GET /api/v4/projects/group%2Fsubgroup%2Fproject/merge_requests/123")
        );
    }
    assert!(requests[3].starts_with(
        "GET /api/v4/projects/5/repository/commits/abc1234/discussions?per_page=100 "
    ));
    assert!(requests[4].starts_with(
        "GET /api/v4/projects/9/repository/commits/abc1234/discussions?per_page=100 "
    ));
    for request in &requests {
        assert!(request.contains("private-token: test-private-token\r\n"));
        assert!(!request.contains("authorization:"));
    }
}

#[tokio::test]
async fn accepts_a_deleted_source_project_and_an_empty_description() {
    let mut merge_request = merge_request();
    merge_request["source_project_id"] = Value::Null;
    merge_request["description"] = Value::Null;
    let server = Server::start(vec![
        Reply::json(merge_request.clone()),
        commits(),
        discussions(),
        Reply::json(json!([{"id": "native", "notes": [note(1)]}])),
        Reply::json(merge_request),
    ])
    .await;
    let input = server
        .client()
        .review_input(&repository(), number())
        .await
        .unwrap();
    assert_eq!(input.source.source_project_id, None);
    assert_eq!(input.context.body.as_deref(), Some(""));
    assert_eq!(input.context.comments.len(), 1);
    assert_eq!(
        input.context.comments[0]
            .commit
            .as_ref()
            .map(CommitHash::as_ref),
        Some("abc1234")
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 5);
    assert!(requests[3].starts_with(
        "GET /api/v4/projects/5/repository/commits/abc1234/discussions?per_page=100 "
    ));
    assert!(
        requests[4]
            .starts_with("GET /api/v4/projects/group%2Fsubgroup%2Fproject/merge_requests/123 ")
    );
}

#[tokio::test]
async fn loads_every_commit_and_discussion_page_without_assuming_commit_order() {
    let server = Server::start(vec![
        Reply::json(merge_request()),
        commits().header("Link: <{base}projects/group%2Fsubgroup%2Fproject/merge_requests/123/commits?per_page=100&page=2>; rel=\"next\""),
        Reply::json(json!([{"id": "2345678"}])),
        Reply::json(json!([{"id": "first", "notes": [note(1)]}])).header("Link: <{base}projects/group%2Fsubgroup%2Fproject/merge_requests/123/discussions?per_page=100&page=2>; rel=\"next\""),
        Reply::json(json!([{"id": "second", "notes": [note(2)]}])),
        discussions(),
        discussions(),
        discussions(),
        discussions(),
        Reply::json(merge_request()),
    ]).await;
    let input = server
        .client()
        .review_input(&repository(), number())
        .await
        .unwrap();
    assert_eq!(
        input
            .commits
            .iter()
            .map(CommitHash::as_ref)
            .collect::<Vec<_>>(),
        ["abc1234", "2345678"]
    );
    assert_eq!(input.context.comments.len(), 2);
    assert!(server.requests()[2].contains("commits?per_page=100&page=2"));
    assert!(server.requests()[4].contains("discussions?per_page=100&page=2"));
}

#[tokio::test]
async fn rejects_unready_refs_before_loading_commits() {
    for refs in [Value::Null, json!({}), json!({"base_sha": "0123456"})] {
        let mut merge_request = merge_request();
        merge_request["diff_refs"] = refs;
        let server = Server::start(vec![Reply::json(merge_request)]).await;
        assert_matches!(
            server.client().review_input(&repository(), number()).await,
            Err(GitLabError::MergeRequestNotReady)
        );
        assert_eq!(server.requests().len(), 1);
    }
}

#[tokio::test]
async fn rejects_a_head_whose_diff_refs_have_not_caught_up() {
    for sha in [Value::Null, json!("def5678")] {
        let mut merge_request = merge_request();
        merge_request["sha"] = sha;
        let server = Server::start(vec![Reply::json(merge_request)]).await;
        assert_matches!(
            server.client().review_input(&repository(), number()).await,
            Err(GitLabError::MergeRequestNotReady)
        );
    }
}

#[tokio::test]
async fn rejects_inconsistent_merge_request_identity() {
    for (field, value) in [
        ("iid", 9876),
        ("project_id", 0),
        ("target_project_id", 9),
        ("source_project_id", 0),
    ] {
        let mut merge_request = merge_request();
        merge_request[field] = json!(value);
        let server = Server::start(vec![Reply::json(merge_request)]).await;
        assert_matches!(
            server.client().review_input(&repository(), number()).await,
            Err(GitLabError::InvalidMergeRequest)
        );
    }
}

#[tokio::test]
async fn revalidates_all_diff_refs_project_identity_and_branch_names() {
    for field in [
        "base_sha",
        "head_sha",
        "start_sha",
        "project",
        "source_project_id",
        "source_branch",
        "target_branch",
    ] {
        let mut current = merge_request();
        match field {
            "base_sha" | "start_sha" => current["diff_refs"][field] = json!("def5678"),
            "head_sha" => {
                current["diff_refs"][field] = json!("def5678");
                current["sha"] = json!("def5678");
            }
            "project" => {
                current["project_id"] = json!(7);
                current["target_project_id"] = json!(7);
            }
            "source_project_id" => current[field] = json!(10),
            _ => current[field] = json!("another-branch"),
        }
        let server = Server::start(vec![
            Reply::json(merge_request()),
            commits(),
            discussions(),
            discussions(),
            discussions(),
            Reply::json(current),
        ])
        .await;
        assert_matches!(
            server.client().review_input(&repository(), number()).await,
            Err(GitLabError::MergeRequestChanged)
        );
        assert_eq!(server.requests().len(), 6, "{field}");
        assert!(
            server.requests()[5]
                .starts_with("GET /api/v4/projects/group%2Fsubgroup%2Fproject/merge_requests/123 ")
        );
    }
}

#[tokio::test]
async fn rejects_missing_duplicate_or_empty_commit_lists() {
    for commits in [
        json!([]),
        json!([{"id": "def5678"}]),
        json!([{"id": "abc1234"}, {"id": "abc1234"}]),
    ] {
        let server = Server::start(vec![Reply::json(merge_request()), Reply::json(commits)]).await;
        assert_matches!(
            server.client().review_input(&repository(), number()).await,
            Err(GitLabError::IncompleteCommits)
        );
        assert_eq!(server.requests().len(), 2);
    }
}

#[tokio::test]
async fn rejects_invalid_hashes_at_the_api_boundary() {
    let server = Server::start(vec![
        Reply::json(merge_request()),
        Reply::json(json!([{"id": "not-a-hash"}])),
    ])
    .await;
    assert_matches!(
        server.client().review_input(&repository(), number()).await,
        Err(GitLabError::Decode { .. })
    );
}

#[tokio::test]
async fn a_later_commit_page_failure_discards_the_input() {
    let server = Server::start(vec![
        Reply::json(merge_request()),
        commits().header("Link: <{base}projects/group%2Fsubgroup%2Fproject/merge_requests/123/commits?per_page=100&page=2>; rel=\"next\""),
        Reply::json(json!({})).status(500),
    ])
    .await;
    assert_matches!(
        server.client().review_input(&repository(), number()).await,
        Err(GitLabError::Api { status: 500, .. })
    );
    assert_eq!(server.requests().len(), 3);
    assert!(server.requests()[2].contains("commits?per_page=100&page=2"));
}

#[tokio::test]
async fn a_later_discussion_page_failure_discards_the_input() {
    let server = Server::start(vec![
        Reply::json(merge_request()),
        commits(),
        discussions().header("Link: <{base}projects/group%2Fsubgroup%2Fproject/merge_requests/123/discussions?per_page=100&page=2>; rel=\"next\""),
        Reply::json(json!({})).status(403),
    ])
    .await;
    assert_matches!(
        server.client().review_input(&repository(), number()).await,
        Err(GitLabError::Api { status: 403, .. })
    );
    assert_eq!(server.requests().len(), 4);
    assert!(server.requests()[3].contains("discussions?per_page=100&page=2"));
}

#[tokio::test]
async fn revalidation_failure_discards_the_input() {
    let server = Server::start(vec![
        Reply::json(merge_request()),
        commits(),
        discussions(),
        discussions(),
        discussions(),
        Reply::json(json!({})).status(503),
    ])
    .await;
    assert_matches!(
        server.client().review_input(&repository(), number()).await,
        Err(GitLabError::Api { status: 503, .. })
    );
    assert_eq!(server.requests().len(), 6);
}

mod commit_comments;

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

#[test]
fn source_snapshot_round_trips_with_its_provider_tag() {
    let merge_request: MergeRequest = serde_json::from_value(merge_request()).unwrap();
    let source = merge_request.source(number()).unwrap();
    let value = serde_json::to_value(&source).unwrap();
    assert_eq!(value["provider"], "gitlab");
    assert_eq!(
        serde_json::from_value::<GitLabReviewSource>(value).unwrap(),
        source
    );
}

#[test]
fn source_snapshot_requires_the_gitlab_provider_and_known_fields() {
    let merge_request: MergeRequest = serde_json::from_value(merge_request()).unwrap();
    let source = merge_request.source(number()).unwrap();
    for provider in [json!("github"), Value::Null] {
        let mut value = serde_json::to_value(&source).unwrap();
        value["provider"] = provider;
        assert_matches!(serde_json::from_value::<GitLabReviewSource>(value), Err(_));
    }
    let mut value = serde_json::to_value(&source).unwrap();
    value.as_object_mut().unwrap().remove("provider");
    assert_matches!(serde_json::from_value::<GitLabReviewSource>(value), Err(_));
    let mut value = serde_json::to_value(&source).unwrap();
    value["unknown"] = json!(true);
    assert_matches!(serde_json::from_value::<GitLabReviewSource>(value), Err(_));
}

#[tokio::test]
async fn sends_post_requests_with_json_bodies() {
    let server = Server::start(vec![Reply::json(json!({"id": 123})).status(201)]).await;
    server
        .client()
        .post::<Value>("projects/5/notes", &json!({"body": "A note"}))
        .await
        .unwrap();
    let requests = server.requests();
    assert!(requests[0].starts_with("POST /api/v4/projects/5/notes "));
    assert!(requests[0].ends_with("{\"body\":\"A note\"}"));
}

#[tokio::test]
async fn decodes_created_post_responses() {
    let server = Server::start(vec![Reply::json(json!({"id": 123})).status(201)]).await;
    let reply: Value = server
        .client()
        .post("projects/5/notes", &json!({"body": "A note"}))
        .await
        .unwrap();
    assert_eq!(reply["id"], 123);
}
