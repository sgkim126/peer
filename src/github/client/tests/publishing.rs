use super::*;
use crate::render::{RenderInput, github};

fn finding() -> RenderInput {
    serde_json::from_value(json!({
        "commit": "abc1234",
        "severity": "high",
        "message": "Check @team <script>.",
        "file": "src/main.rs",
        "line": 5,
    }))
    .unwrap()
}

#[tokio::test]
async fn empty_documents_do_not_create_comments() {
    let input = serde_json::from_value(json!({
        "ordered_commits": [],
        "stages": []
    }))
    .unwrap();
    let server = Server::start(vec![pull()]).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.urls, Vec::<String>::new());
    assert_eq!(server.requests().len(), 1);
}

fn created() -> Reply {
    let mut reply = Reply::json(json!({
        "html_url": "https://github.com/owner/repo/pull/123#issuecomment-1"
    }));
    reply.status = 201;
    reply
}

fn request_body(request: &str) -> Value {
    serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap()
}

#[tokio::test]
async fn publishes_rendered_input_to_the_selected_pull_request() {
    let server = Server::start(vec![pull(), created()]).await;
    let input = finding();
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();

    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].starts_with("GET /repos/owner/repo/pulls/123 "));
    assert!(requests[1].starts_with("POST /repos/owner/repo/issues/123/comments "));
    assert_eq!(
        request_body(&requests[1]),
        json!({
            "body": github::render(&input, "owner/repo")
        })
    );
    assert_eq!(report.urls.len(), 1);
    assert!(report.to_string().contains("Published 1 comment(s)."));
}

#[tokio::test]
async fn does_not_publish_when_the_pull_request_cannot_be_loaded() {
    let mut reply = pull();
    reply.status = 404;
    let server = Server::start(vec![reply]).await;
    assert_matches!(
        server
            .client()
            .publish(&repository(), number(), &finding())
            .await,
        Err(GitHubError::Api { status: 404, .. })
    );
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn reports_comment_creation_failure_without_retrying() {
    let mut reply = created();
    reply.status = 403;
    let server = Server::start(vec![pull(), reply]).await;
    assert_matches!(
        server
            .client()
            .publish(&repository(), number(), &finding())
            .await,
        Err(GitHubError::Api { status: 403, .. })
    );
    assert_eq!(server.requests().len(), 2);
}
