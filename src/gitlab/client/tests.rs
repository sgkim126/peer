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
    assert_eq!(requests.len(), 4);
    for request in &requests {
        assert!(
            request
                .starts_with("GET /api/v4/projects/group%2Fsubgroup%2Fproject/merge_requests/123")
        );
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
}

#[tokio::test]
async fn loads_every_commit_and_discussion_page_without_assuming_commit_order() {
    let server = Server::start(vec![
        Reply::json(merge_request()),
        commits().header("Link: <{base}projects/group%2Fsubgroup%2Fproject/merge_requests/123/commits?per_page=100&page=2>; rel=\"next\""),
        Reply::json(json!([{"id": "2345678"}])),
        Reply::json(json!([{"id": "first", "notes": [note(1)]}])).header("Link: <{base}projects/group%2Fsubgroup%2Fproject/merge_requests/123/discussions?per_page=100&page=2>; rel=\"next\""),
        Reply::json(json!([{"id": "second", "notes": [note(2)]}])),
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
            Reply::json(current),
        ])
        .await;
        assert_matches!(
            server.client().review_input(&repository(), number()).await,
            Err(GitLabError::MergeRequestChanged)
        );
        assert_eq!(server.requests().len(), 4, "{field}");
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
        Reply::json(json!({})).status(503),
    ])
    .await;
    assert_matches!(
        server.client().review_input(&repository(), number()).await,
        Err(GitLabError::Api { status: 503, .. })
    );
}

#[tokio::test]
async fn rejects_pagination_links_to_another_origin() {
    let server = Server::start(vec![Reply::json(json!([])).header(
        "Link: <https://example.com/api/v4/projects/5/notes?page=2>; rel=\"next\"",
    )])
    .await;

    assert_matches!(
        server.client().list::<Value>("projects/5/notes").await,
        Err(GitLabError::InvalidPagination)
    );
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn rejects_pagination_links_to_another_collection() {
    let server =
        Server::start(vec![Reply::json(json!([])).header(
            "Link: <{base}projects/another-project/notes?page=2>; rel=\"next\"",
        )])
        .await;

    assert_matches!(
        server.client().list::<Value>("projects/5/notes").await,
        Err(GitLabError::InvalidPagination)
    );
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn rejects_pagination_links_to_another_origin_with_credentials() {
    let server = Server::start(vec![Reply::json(json!([])).header(
        "Link: <http://user:password@localhost/api/v4/projects/5/notes?page=2>; rel=\"next\"",
    )])
    .await;

    assert_matches!(
        server.client().list::<Value>("projects/5/notes").await,
        Err(GitLabError::InvalidPagination)
    );
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn rejects_pagination_links_with_fragments() {
    let server =
        Server::start(vec![Reply::json(json!([])).header(
            "Link: <{base}projects/5/notes?page=2#fragment>; rel=\"next\"",
        )])
        .await;

    assert_matches!(
        server.client().list::<Value>("projects/5/notes").await,
        Err(GitLabError::InvalidPagination)
    );
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn rejects_pagination_links_back_to_an_already_visited_page() {
    let server = Server::start(vec![
        Reply::json(json!([])).header("Link: <{base}projects/5/notes?per_page=100>; rel=\"next\""),
        Reply::json(json!([])),
    ])
    .await;

    assert_matches!(
        server.client().list::<Value>("projects/5/notes").await,
        Err(GitLabError::InvalidPagination)
    );
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn rejects_repeated_next_page_links() {
    let server = Server::start(vec![
        Reply::json(json!([])).header("Link: <{base}projects/5/notes?page=2>; rel=\"next\""),
        Reply::json(json!([])).header("Link: <{base}projects/5/notes?page=2>; rel=\"next\""),
    ])
    .await;

    assert_matches!(
        server.client().list::<Value>("projects/5/notes").await,
        Err(GitLabError::InvalidPagination)
    );
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn rejects_next_links_without_an_angle_bracketed_url() {
    let mut headers = HeaderMap::new();
    headers.insert(LINK, HeaderValue::from_static("not-a-url; rel=next"));

    assert_matches!(next_page(&headers), Err(GitLabError::InvalidPagination));
}

#[test]
fn rejects_relative_next_page_links() {
    let mut headers = HeaderMap::new();
    headers.insert(LINK, HeaderValue::from_static("<relative-path>; rel=next"));

    assert_matches!(next_page(&headers), Err(GitLabError::InvalidPagination));
}

#[test]
fn rejects_multiple_next_page_links() {
    let mut headers = HeaderMap::new();
    headers.insert(
        LINK,
        HeaderValue::from_static(
            "<https://gitlab.com/one>; rel=next, <https://gitlab.com/two>; rel=next",
        ),
    );

    assert_matches!(next_page(&headers), Err(GitLabError::InvalidPagination));
}

#[tokio::test]
async fn collects_items_from_all_linked_pages() {
    let server = Server::start(vec![
        Reply::json(json!([1, 2])).header("Link: <{base}projects/5/notes?page=2>; rel=\"next\""),
        Reply::json(json!([3])).header("Link: <{base}projects/5/notes?page=3>; rel=\"next\""),
        Reply::json(json!([4])),
    ])
    .await;

    let items = server
        .client()
        .list::<Value>("projects/5/notes")
        .await
        .unwrap();

    assert_eq!(items, vec![json!(1), json!(2), json!(3), json!(4)]);
    assert_eq!(server.requests().len(), 3);
}

#[tokio::test]
async fn preserves_initial_query_parameters() {
    let server = Server::start(vec![Reply::json(json!([]))]).await;

    server
        .client()
        .list::<Value>("projects/5/notes?sort=asc")
        .await
        .unwrap();

    assert!(
        server.requests()[0].starts_with("GET /api/v4/projects/5/notes?sort=asc&per_page=100 ")
    );
}

#[tokio::test]
async fn preserves_server_provided_next_urls() {
    let server = Server::start(vec![
        Reply::json(json!([1])).header(concat!(
            "Link: <{base}projects/5/notes?",
            "pagination=keyset&cursor=opaque%2Bvalue%2Fpart%3D&per_page=2&sort=desc>; rel=\"next\"",
        )),
        Reply::json(json!([2])),
    ])
    .await;

    server
        .client()
        .list::<Value>("projects/5/notes?sort=asc")
        .await
        .unwrap();

    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].starts_with(concat!(
        "GET /api/v4/projects/5/notes?",
        "pagination=keyset&cursor=opaque%2Bvalue%2Fpart%3D&per_page=2&sort=desc ",
    )));
}

#[tokio::test]
async fn stops_when_link_header_is_missing() {
    let server = Server::start(vec![Reply::json(json!([1])), Reply::json(json!([2]))]).await;

    let items = server
        .client()
        .list::<Value>("projects/5/notes")
        .await
        .unwrap();

    assert_eq!(items, vec![json!(1)]);
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn ignores_empty_x_next_page() {
    let next = "https://gitlab.com/api/v4/projects/5/notes?page=2";
    let mut headers = HeaderMap::new();
    headers.insert(
        LINK,
        HeaderValue::from_str(&format!("<{next}>; rel=next")).unwrap(),
    );
    headers.insert("x-next-page", HeaderValue::from_static(""));

    assert_eq!(
        next_page(&headers).unwrap(),
        Some(Url::parse(next).unwrap())
    );
}

#[test]
fn ignores_whitespace_only_x_next_page() {
    let next = "https://gitlab.com/api/v4/projects/5/notes?page=2";
    let mut headers = HeaderMap::new();
    headers.insert(
        LINK,
        HeaderValue::from_str(&format!("<{next}>; rel=next")).unwrap(),
    );
    headers.insert("x-next-page", HeaderValue::from_static(" \t "));

    assert_eq!(
        next_page(&headers).unwrap(),
        Some(Url::parse(next).unwrap())
    );
}

#[test]
fn ignores_conflicting_x_next_page() {
    let next = "https://gitlab.com/api/v4/projects/5/notes?page=2";
    let mut headers = HeaderMap::new();
    headers.insert(
        LINK,
        HeaderValue::from_str(&format!("<{next}>; rel=next")).unwrap(),
    );
    headers.insert("x-next-page", HeaderValue::from_static("3"));

    assert_eq!(
        next_page(&headers).unwrap(),
        Some(Url::parse(next).unwrap())
    );
}

#[test]
fn ignores_non_numeric_x_next_page() {
    let next = "https://gitlab.com/api/v4/projects/5/notes?page=2";
    let mut headers = HeaderMap::new();
    headers.insert(
        LINK,
        HeaderValue::from_str(&format!("<{next}>; rel=next")).unwrap(),
    );
    headers.insert("x-next-page", HeaderValue::from_static("invalid"));

    assert_eq!(
        next_page(&headers).unwrap(),
        Some(Url::parse(next).unwrap())
    );
}

#[test]
fn ignores_x_next_page_without_link() {
    let mut headers = HeaderMap::new();
    headers.insert("x-next-page", HeaderValue::from_static("2"));

    assert_eq!(next_page(&headers).unwrap(), None);
}

#[test]
fn ignores_previous_page_links() {
    let mut headers = HeaderMap::new();
    headers.insert(
        LINK,
        HeaderValue::from_static("<https://gitlab.com/api/v4/projects/5/notes?page=1>; rel=prev"),
    );

    assert_eq!(next_page(&headers).unwrap(), None);
}

#[test]
fn ignores_first_page_links() {
    let mut headers = HeaderMap::new();
    headers.insert(
        LINK,
        HeaderValue::from_static("<https://gitlab.com/api/v4/projects/5/notes?page=1>; rel=first"),
    );

    assert_eq!(next_page(&headers).unwrap(), None);
}

#[test]
fn ignores_last_page_links() {
    let mut headers = HeaderMap::new();
    headers.insert(
        LINK,
        HeaderValue::from_static("<https://gitlab.com/api/v4/projects/5/notes?page=3>; rel=last"),
    );

    assert_eq!(next_page(&headers).unwrap(), None);
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

#[tokio::test]
async fn bad_request_responses_do_not_imply_publication() {
    let server = Server::start(vec![Reply::json(json!({})).status(400)]).await;
    let error = server
        .client()
        .post::<Value>("projects/5/notes", &json!({"body": "A note"}))
        .await
        .unwrap_err();
    assert_matches!(error, GitLabError::Api { status: 400, .. });
    assert!(!error.may_have_published());
}

#[tokio::test]
async fn request_timeout_responses_may_have_published() {
    let server = Server::start(vec![Reply::json(json!({})).status(408)]).await;
    let error = server
        .client()
        .post::<Value>("projects/5/notes", &json!({"body": "A note"}))
        .await
        .unwrap_err();
    assert_matches!(error, GitLabError::Api { status: 408, .. });
    assert!(error.may_have_published());
}

#[tokio::test]
async fn server_error_responses_may_have_published() {
    let server = Server::start(vec![Reply::json(json!({})).status(500)]).await;
    let error = server
        .client()
        .post::<Value>("projects/5/notes", &json!({"body": "A note"}))
        .await
        .unwrap_err();
    assert_matches!(error, GitLabError::Api { status: 500, .. });
    assert!(error.may_have_published());
}

#[tokio::test]
async fn undecodable_created_responses_may_have_published() {
    let server = Server::start(vec![Reply::json(json!({})).status(201)]).await;
    #[derive(Debug, Deserialize)]
    struct Posted {
        #[serde(rename = "id")]
        _id: u64,
    }
    let error = server
        .client()
        .post::<Posted>("projects/5/notes", &json!({"body": "A note"}))
        .await
        .unwrap_err();
    assert_matches!(error, GitLabError::Decode { .. });
    assert!(error.may_have_published());
}

#[test]
fn recognizes_position_field_validation_errors() {
    let message = json!({
        "position": ["is invalid"]
    });

    assert!(position_error(&message));
}

#[test]
fn recognizes_dotted_position_validation_errors() {
    let message = json!({
        "position.head_sha": ["is invalid"]
    });

    assert!(position_error(&message));
}

#[test]
fn recognizes_errors_with_multiple_position_fields() {
    let message = json!({
        "position": ["is invalid"],
        "line_code": ["can't be blank"]
    });

    assert!(position_error(&message));
}

#[test]
fn recognizes_bracketed_position_validation_errors() {
    let message = json!({
        "position[head_sha]": "is invalid"
    });

    assert!(position_error(&message));
}

#[test]
fn recognizes_nested_position_validation_errors() {
    let message = json!({
        "position": {
            "new_line": ["is invalid"]
        }
    });

    assert!(position_error(&message));
}

#[test]
fn recognizes_line_code_validation_errors() {
    let message = json!({
        "line_code": ["can't be blank"]
    });

    assert!(position_error(&message));
}

#[test]
fn recognizes_multiple_validation_messages_for_a_position_field() {
    let message = json!({
        "position": ["is invalid", "is incomplete"]
    });

    assert!(position_error(&message));
}

#[test]
fn recognizes_commit_id_errors_that_mention_diff_refs() {
    let message = json!({
        "commit_id": ["does not match the diff refs"]
    });

    assert!(position_error(&message));
}

#[test]
fn recognizes_line_code_errors_in_stringified_ruby_hashes() {
    let message = json!("400 Bad request - Note {:line_code=>[\"can't be blank\"]}");

    assert!(position_error(&message));
}

#[test]
fn recognizes_position_errors_in_quoted_ruby_hashes() {
    let message = json!("400 (Bad request) \"Note {:position=>[\"is incomplete\"]}\" not given");

    assert!(position_error(&message));
}

#[test]
fn recognizes_commit_diff_refs_errors_in_quoted_ruby_hashes() {
    let message = json!(
        "400 (Bad request) \"Note {:commit_id=>[\"does not match the diff refs\"]}\" not given"
    );

    assert!(position_error(&message));
}

#[test]
fn rejects_null_position_error_messages() {
    assert!(!position_error(&Value::Null));
}

#[test]
fn rejects_empty_position_error_objects() {
    let message = json!({});

    assert!(!position_error(&message));
}

#[test]
fn rejects_position_fields_with_empty_validation_arrays() {
    let message = json!({
        "position": []
    });

    assert!(!position_error(&message));
}

#[test]
fn rejects_position_fields_with_null_validation_details() {
    let message = json!({
        "position": null
    });

    assert!(!position_error(&message));
}

#[test]
fn rejects_mixed_position_and_body_validation_errors() {
    let message = json!({
        "position": ["is invalid"],
        "body": ["can't be blank"]
    });

    assert!(!position_error(&message));
}

#[test]
fn rejects_body_errors_that_only_mention_position() {
    let message = json!({
        "body": ["contains the word position"]
    });

    assert!(!position_error(&message));
}

#[test]
fn rejects_field_names_that_only_start_with_position() {
    let message = json!({
        "positioning": ["is invalid"]
    });

    assert!(!position_error(&message));
}

#[test]
fn rejects_commit_id_errors_without_diff_refs() {
    let message = json!({
        "commit_id": ["is invalid"]
    });

    assert!(!position_error(&message));
}

#[test]
fn rejects_position_error_messages_in_arrays() {
    let message = json!(["position is invalid"]);

    assert!(!position_error(&message));
}

#[test]
fn rejects_plain_text_position_validation_errors() {
    let message = json!("position is invalid");

    assert!(!position_error(&message));
}

#[test]
fn rejects_plain_text_bracketed_position_errors() {
    let message = json!("position[new_line] is missing");

    assert!(!position_error(&message));
}

#[test]
fn rejects_bad_request_text_without_a_ruby_hash() {
    let message = json!("400 Bad request - position is incomplete");

    assert!(!position_error(&message));
}

#[test]
fn rejects_plain_text_body_errors_that_mention_position() {
    let message = json!("body contains an invalid position");

    assert!(!position_error(&message));
}

#[test]
fn rejects_plain_text_mixed_position_and_body_errors() {
    let message = json!("position is invalid and body can't be blank");

    assert!(!position_error(&message));
}

#[test]
fn rejects_bare_bad_request_error_strings() {
    let message = json!("400 Bad request");

    assert!(!position_error(&message));
}

#[test]
fn rejects_mixed_position_and_body_errors_in_ruby_hashes() {
    let message =
        json!("400 Bad request - Note {:position=>[\"is invalid\"], :body=>[\"can't be blank\"]}");

    assert!(!position_error(&message));
}

#[test]
fn rejects_non_diff_refs_commit_errors_in_ruby_hashes() {
    let message = json!(
        "400 Bad request - Note {:line_code=>[\"is invalid\"], :commit_id=>[\"is invalid\"]}"
    );

    assert!(!position_error(&message));
}

#[test]
fn rejects_empty_stringified_ruby_hashes() {
    let message = json!("400 Bad request - Note {}");

    assert!(!position_error(&message));
}

#[test]
fn rejects_stringified_ruby_hashes_without_a_closing_brace() {
    let message = json!("400 Bad request - Note {:position=>[\"is incomplete\"]");

    assert!(!position_error(&message));
}

#[test]
fn recognizes_commit_target_string_validation_messages() {
    let message = json!({"commit_id": "is invalid"});

    assert!(commit_error(&message));
}

#[test]
fn recognizes_multiple_commit_target_validation_messages() {
    let message = json!({"commit_id": ["is invalid", "does not exist"]});

    assert!(commit_error(&message));
}

#[test]
fn recognizes_commit_target_diff_refs_validation_errors() {
    let message = json!({"commit_id": ["does not match the diff refs"]});

    assert!(commit_error(&message));
}

#[test]
fn recognizes_commit_target_errors_in_stringified_ruby_hashes() {
    let message = json!("400 Bad request - Note {:commit_id=>[\"is invalid\"]}");

    assert!(commit_error(&message));
}

#[test]
fn recognizes_commit_target_errors_in_quoted_ruby_hashes() {
    let message = json!("400 (Bad request) \"Note {:commit_id=>[\"is invalid\"]}\" not given");

    assert!(commit_error(&message));
}

#[test]
fn recognizes_colons_in_stringified_commit_target_validation_messages() {
    let message = json!("400 Bad request - Note {:commit_id=>[\"does not exist: invalid SHA\"]}");

    assert!(commit_error(&message));
}

#[test]
fn rejects_commit_target_errors_mixed_with_body_errors() {
    let message = json!({"commit_id": ["is invalid"], "body": ["is invalid"]});

    assert!(!commit_error(&message));
}

#[test]
fn rejects_commit_target_errors_mixed_with_body_errors_in_ruby_hashes() {
    let message =
        json!("400 Bad request - Note {:commit_id=>[\"is invalid\"], :body=>[\"is invalid\"]}");

    assert!(!commit_error(&message));
}

#[test]
fn rejects_commit_target_errors_mixed_with_author_errors() {
    let message = json!({"commit_id": ["is invalid"], "author": ["is invalid"]});

    assert!(!commit_error(&message));
}

#[test]
fn rejects_commit_target_errors_mixed_with_author_errors_in_ruby_hashes() {
    let message =
        json!("400 Bad request - Note {:commit_id=>[\"is invalid\"], :author=>[\"is invalid\"]}");

    assert!(!commit_error(&message));
}

#[test]
fn rejects_commit_target_errors_mixed_with_position_errors() {
    let message = json!({"commit_id": ["is invalid"], "position": ["is invalid"]});

    assert!(!commit_error(&message));
}

#[test]
fn rejects_commit_target_errors_mixed_with_position_errors_in_ruby_hashes() {
    let message =
        json!("400 Bad request - Note {:commit_id=>[\"is invalid\"], :position=>[\"is invalid\"]}");

    assert!(!commit_error(&message));
}

#[test]
fn rejects_commit_target_errors_mixed_with_line_code_errors() {
    let message = json!({"commit_id": ["is invalid"], "line_code": ["is invalid"]});

    assert!(!commit_error(&message));
}

#[test]
fn rejects_commit_target_errors_mixed_with_line_code_errors_in_ruby_hashes() {
    let message = json!(
        "400 Bad request - Note {:commit_id=>[\"is invalid\"], :line_code=>[\"is invalid\"]}"
    );

    assert!(!commit_error(&message));
}

#[test]
fn rejects_null_commit_target_validation_details() {
    let message = json!({"commit_id": null});

    assert!(!commit_error(&message));
}

#[test]
fn rejects_null_commit_target_validation_details_in_ruby_hashes() {
    let message = json!("400 Bad request - Note {:commit_id=>null}");

    assert!(!commit_error(&message));
}

#[test]
fn rejects_empty_commit_target_validation_strings() {
    let message = json!({"commit_id": ""});

    assert!(!commit_error(&message));
}

#[test]
fn rejects_empty_commit_target_validation_strings_in_ruby_hashes() {
    let message = json!("400 Bad request - Note {:commit_id=>\"\"}");

    assert!(!commit_error(&message));
}

#[test]
fn rejects_whitespace_only_commit_target_validation_strings() {
    let message = json!({"commit_id": " \t"});

    assert!(!commit_error(&message));
}

#[test]
fn rejects_whitespace_only_commit_target_validation_strings_in_ruby_hashes() {
    let message = json!("400 Bad request - Note {:commit_id=>\" \\t\"}");

    assert!(!commit_error(&message));
}

#[test]
fn rejects_empty_commit_target_validation_arrays() {
    let message = json!({"commit_id": []});

    assert!(!commit_error(&message));
}

#[test]
fn rejects_empty_commit_target_validation_arrays_in_ruby_hashes() {
    let message = json!("400 Bad request - Note {:commit_id=>[]}");

    assert!(!commit_error(&message));
}

#[test]
fn rejects_commit_target_validation_arrays_containing_empty_strings() {
    let message = json!({"commit_id": [""]});

    assert!(!commit_error(&message));
}

#[test]
fn rejects_commit_target_validation_arrays_containing_empty_strings_in_ruby_hashes() {
    let message = json!("400 Bad request - Note {:commit_id=>[\"\"]}");

    assert!(!commit_error(&message));
}

#[test]
fn rejects_commit_target_validation_arrays_mixing_valid_and_whitespace_messages() {
    let message = json!({"commit_id": ["is invalid", " "]});

    assert!(!commit_error(&message));
}

#[test]
fn rejects_commit_target_validation_arrays_mixing_valid_and_whitespace_messages_in_ruby_hashes() {
    let message = json!("400 Bad request - Note {:commit_id=>[\"is invalid\",\" \"]}");

    assert!(!commit_error(&message));
}

#[test]
fn rejects_empty_commit_target_validation_objects() {
    let message = json!({"commit_id": {}});

    assert!(!commit_error(&message));
}

#[test]
fn rejects_empty_commit_target_validation_objects_in_ruby_hashes() {
    let message = json!("400 Bad request - Note {:commit_id=>{}}");

    assert!(!commit_error(&message));
}

#[test]
fn rejects_boolean_commit_target_validation_details() {
    let message = json!({"commit_id": false});

    assert!(!commit_error(&message));
}

#[test]
fn rejects_boolean_commit_target_validation_details_in_ruby_hashes() {
    let message = json!("400 Bad request - Note {:commit_id=>false}");

    assert!(!commit_error(&message));
}

#[test]
fn rejects_null_commit_target_error_messages() {
    assert!(!commit_error(&Value::Null));
}

#[test]
fn rejects_empty_commit_target_error_objects() {
    let message = json!({});

    assert!(!commit_error(&message));
}

#[test]
fn rejects_field_names_that_only_start_with_commit_id() {
    let message = json!({"commit_id_suffix": ["is invalid"]});

    assert!(!commit_error(&message));
}

#[test]
fn rejects_body_errors_that_only_mention_commit_id() {
    let message = json!({"body": ["commit_id is invalid"]});

    assert!(!commit_error(&message));
}

#[test]
fn rejects_commit_target_error_messages_in_arrays() {
    let message = json!(["commit_id is invalid"]);

    assert!(!commit_error(&message));
}

#[test]
fn rejects_plain_text_commit_target_validation_errors() {
    let message = json!("commit_id is invalid");

    assert!(!commit_error(&message));
}

#[test]
fn rejects_empty_stringified_ruby_hashes_as_commit_target_errors() {
    let message = json!("400 Bad request - Note {}");

    assert!(!commit_error(&message));
}

#[test]
fn rejects_commit_target_ruby_hashes_without_a_closing_brace() {
    let message = json!("400 Bad request - Note {:commit_id=>[\"is invalid\"]");

    assert!(!commit_error(&message));
}

#[test]
fn rejects_commit_target_ruby_hashes_without_validation_details() {
    let message = json!("400 Bad request - Note {:commit_id=>}");

    assert!(!commit_error(&message));
}

async fn commit_target_validation_response(status: u16) -> GitLabError {
    let server = Server::start(vec![
        Reply::json(json!({"message": {"commit_id": ["is invalid"]}})).status(status),
    ])
    .await;

    server
        .client()
        .post::<Value>(
            "projects/5/merge_requests/1/discussions",
            &json!({"body": "A note", "commit_id": "abc1234"}),
        )
        .await
        .unwrap_err()
}

#[tokio::test]
async fn bad_request_responses_can_report_commit_target_failures() {
    let error = commit_target_validation_response(400).await;

    assert_matches!(
        error,
        GitLabError::Api {
            status: 400,
            position_invalid: false,
            commit_invalid: true,
            ..
        }
    );
}

#[tokio::test]
async fn unprocessable_entity_responses_can_report_commit_target_failures() {
    let error = commit_target_validation_response(422).await;

    assert_matches!(
        error,
        GitLabError::Api {
            status: 422,
            position_invalid: false,
            commit_invalid: true,
            ..
        }
    );
}

#[tokio::test]
async fn unauthorized_responses_are_not_commit_target_failures() {
    let error = commit_target_validation_response(401).await;

    assert_matches!(
        error,
        GitLabError::Api {
            status: 401,
            position_invalid: false,
            commit_invalid: false,
            ..
        }
    );
}

#[tokio::test]
async fn forbidden_responses_are_not_commit_target_failures() {
    let error = commit_target_validation_response(403).await;

    assert_matches!(
        error,
        GitLabError::Api {
            status: 403,
            position_invalid: false,
            commit_invalid: false,
            ..
        }
    );
}

#[tokio::test]
async fn not_found_responses_are_not_commit_target_failures() {
    let error = commit_target_validation_response(404).await;

    assert_matches!(
        error,
        GitLabError::Api {
            status: 404,
            position_invalid: false,
            commit_invalid: false,
            ..
        }
    );
}

#[tokio::test]
async fn request_timeout_responses_are_not_commit_target_failures() {
    let error = commit_target_validation_response(408).await;

    assert_matches!(
        error,
        GitLabError::Api {
            status: 408,
            position_invalid: false,
            commit_invalid: false,
            ..
        }
    );
}

#[tokio::test]
async fn too_many_requests_responses_are_not_commit_target_failures() {
    let error = commit_target_validation_response(429).await;

    assert_matches!(
        error,
        GitLabError::Api {
            status: 429,
            position_invalid: false,
            commit_invalid: false,
            ..
        }
    );
}

#[tokio::test]
async fn internal_server_error_responses_are_not_commit_target_failures() {
    let error = commit_target_validation_response(500).await;

    assert_matches!(
        error,
        GitLabError::Api {
            status: 500,
            position_invalid: false,
            commit_invalid: false,
            ..
        }
    );
}

#[tokio::test]
async fn service_unavailable_responses_are_not_commit_target_failures() {
    let error = commit_target_validation_response(503).await;

    assert_matches!(
        error,
        GitLabError::Api {
            status: 503,
            position_invalid: false,
            commit_invalid: false,
            ..
        }
    );
}

#[tokio::test]
async fn mixed_target_and_body_response_errors_do_not_permit_fallback() {
    let server = Server::start(vec![
        Reply::json(json!({
            "message": {
                "commit_id": ["is invalid"],
                "body": ["can't be blank"]
            }
        }))
        .status(422),
    ])
    .await;
    let error = server
        .client()
        .post::<Value>(
            "projects/5/merge_requests/1/discussions",
            &json!({"body": "A note", "commit_id": "abc1234"}),
        )
        .await
        .unwrap_err();

    assert_matches!(
        error,
        GitLabError::Api {
            position_invalid: false,
            commit_invalid: false,
            ..
        }
    );
}

#[tokio::test]
async fn bad_request_responses_can_report_position_failures() {
    let server = Server::start(vec![
        Reply::json(json!({
            "message": {
                "position": ["is invalid"]
            }
        }))
        .status(400),
    ])
    .await;
    let error = server
        .client()
        .post::<Value>(
            "projects/5/notes",
            &json!({
                "body": "A note"
            }),
        )
        .await
        .unwrap_err();

    assert_matches!(
        error,
        GitLabError::Api {
            position_invalid: true,
            ..
        }
    );
}

#[tokio::test]
async fn unprocessable_entity_responses_can_report_position_failures() {
    let server = Server::start(vec![
        Reply::json(json!({
            "message": {
                "position": ["is invalid"]
            }
        }))
        .status(422),
    ])
    .await;
    let error = server
        .client()
        .post::<Value>(
            "projects/5/notes",
            &json!({
                "body": "A note"
            }),
        )
        .await
        .unwrap_err();

    assert_matches!(
        error,
        GitLabError::Api {
            position_invalid: true,
            ..
        }
    );
}

#[tokio::test]
async fn unauthorized_responses_are_not_position_failures() {
    let server = Server::start(vec![
        Reply::json(json!({
            "message": {
                "position": ["is invalid"]
            }
        }))
        .status(401),
    ])
    .await;
    let error = server
        .client()
        .post::<Value>(
            "projects/5/notes",
            &json!({
                "body": "A note"
            }),
        )
        .await
        .unwrap_err();

    assert_matches!(
        error,
        GitLabError::Api {
            position_invalid: false,
            ..
        }
    );
}

#[tokio::test]
async fn forbidden_responses_are_not_position_failures() {
    let server = Server::start(vec![
        Reply::json(json!({
            "message": {
                "position": ["is invalid"]
            }
        }))
        .status(403),
    ])
    .await;
    let error = server
        .client()
        .post::<Value>(
            "projects/5/notes",
            &json!({
                "body": "A note"
            }),
        )
        .await
        .unwrap_err();

    assert_matches!(
        error,
        GitLabError::Api {
            position_invalid: false,
            ..
        }
    );
}

#[tokio::test]
async fn too_many_requests_responses_are_not_position_failures() {
    let server = Server::start(vec![
        Reply::json(json!({
            "message": {
                "position": ["is invalid"]
            }
        }))
        .status(429),
    ])
    .await;
    let error = server
        .client()
        .post::<Value>(
            "projects/5/notes",
            &json!({
                "body": "A note"
            }),
        )
        .await
        .unwrap_err();

    assert_matches!(
        error,
        GitLabError::Api {
            position_invalid: false,
            ..
        }
    );
}

#[tokio::test]
async fn internal_server_error_responses_are_not_position_failures() {
    let server = Server::start(vec![
        Reply::json(json!({
            "message": {
                "position": ["is invalid"]
            }
        }))
        .status(500),
    ])
    .await;
    let error = server
        .client()
        .post::<Value>(
            "projects/5/notes",
            &json!({
                "body": "A note"
            }),
        )
        .await
        .unwrap_err();

    assert_matches!(
        error,
        GitLabError::Api {
            position_invalid: false,
            ..
        }
    );
}

#[tokio::test]
async fn malformed_error_json_is_not_a_position_failure() {
    let mut reply = Reply::json(json!({})).status(400);
    reply.body = "invalid-json".into();
    let server = Server::start(vec![reply]).await;
    let error = server
        .client()
        .post::<Value>(
            "projects/5/notes",
            &json!({
                "body": "A note"
            }),
        )
        .await
        .unwrap_err();

    assert_matches!(
        error,
        GitLabError::Api {
            position_invalid: false,
            ..
        }
    );
}

#[tokio::test]
async fn body_validation_errors_are_not_position_failures() {
    let server = Server::start(vec![
        Reply::json(json!({
            "message": {
                "body": ["can't be blank"]
            }
        }))
        .status(400),
    ])
    .await;
    let error = server
        .client()
        .post::<Value>(
            "projects/5/notes",
            &json!({
                "body": "A note"
            }),
        )
        .await
        .unwrap_err();

    assert_matches!(
        error,
        GitLabError::Api {
            position_invalid: false,
            ..
        }
    );
}

#[tokio::test]
async fn api_error_debug_omits_malformed_response_bodies() {
    let secret = "private-response-message";
    let mut reply = Reply::json(json!({})).status(400);
    reply.body = format!("invalid-json {secret}");
    let server = Server::start(vec![reply]).await;
    let error = server
        .client()
        .post::<Value>(
            "projects/5/notes",
            &json!({
                "body": "A note"
            }),
        )
        .await
        .unwrap_err();

    assert!(!format!("{error:?}").contains(secret));
}

#[tokio::test]
async fn api_error_display_omits_malformed_response_bodies() {
    let secret = "private-response-message";
    let mut reply = Reply::json(json!({})).status(400);
    reply.body = format!("invalid-json {secret}");
    let server = Server::start(vec![reply]).await;
    let error = server
        .client()
        .post::<Value>(
            "projects/5/notes",
            &json!({
                "body": "A note"
            }),
        )
        .await
        .unwrap_err();

    assert!(!error.to_string().contains(secret));
}

#[tokio::test]
async fn api_error_debug_omits_body_validation_messages() {
    let secret = "private-response-message";
    let server = Server::start(vec![
        Reply::json(json!({
            "message": {
                "body": [secret]
            }
        }))
        .status(400),
    ])
    .await;
    let error = server
        .client()
        .post::<Value>(
            "projects/5/notes",
            &json!({
                "body": "A note"
            }),
        )
        .await
        .unwrap_err();

    assert!(!format!("{error:?}").contains(secret));
}

#[tokio::test]
async fn api_error_display_omits_body_validation_messages() {
    let secret = "private-response-message";
    let server = Server::start(vec![
        Reply::json(json!({
            "message": {
                "body": [secret]
            }
        }))
        .status(400),
    ])
    .await;
    let error = server
        .client()
        .post::<Value>(
            "projects/5/notes",
            &json!({
                "body": "A note"
            }),
        )
        .await
        .unwrap_err();

    assert!(!error.to_string().contains(secret));
}

#[tokio::test]
async fn api_error_debug_omits_position_validation_messages() {
    let secret = "private-response-message";
    let server = Server::start(vec![
        Reply::json(json!({
            "message": {
                "position": [secret]
            }
        }))
        .status(400),
    ])
    .await;
    let error = server
        .client()
        .post::<Value>(
            "projects/5/notes",
            &json!({
                "body": "A note"
            }),
        )
        .await
        .unwrap_err();

    assert!(!format!("{error:?}").contains(secret));
}

#[tokio::test]
async fn api_error_display_omits_position_validation_messages() {
    let secret = "private-response-message";
    let server = Server::start(vec![
        Reply::json(json!({
            "message": {
                "position": [secret]
            }
        }))
        .status(400),
    ])
    .await;
    let error = server
        .client()
        .post::<Value>(
            "projects/5/notes",
            &json!({
                "body": "A note"
            }),
        )
        .await
        .unwrap_err();

    assert!(!error.to_string().contains(secret));
}
