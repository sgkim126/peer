use super::*;

fn review_replies(pull: &Value, commits: &[&str]) -> Vec<Reply> {
    vec![
        Reply::json(pull.clone()),
        Reply::json(json!([comment(100, Value::Null)])),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!(
            commits
                .iter()
                .map(|sha| json!({"sha": sha}))
                .collect::<Vec<_>>()
        )),
    ]
}

fn failure(status: u16) -> Reply {
    let mut reply = Reply::json(json!({}));
    reply.status = status;
    reply
}

fn forbidden(message: &str) -> Reply {
    let mut reply = Reply::json(json!({"message": message}));
    reply.status = 403;
    reply
}

async fn assert_inaccessible_source_is_skipped(reply: Reply) {
    let pull = pull_with_source(Some("contributor/fork"), "def5678", 2);
    let mut replies = review_replies(&pull, &["abc1234", "def5678"]);
    replies.extend([
        Reply::json(json!([commit_comment(1, "abc1234")])),
        reply,
        Reply::json(json!([commit_comment(2, "def5678")])),
        Reply::json(pull),
    ]);
    let server = Server::start(replies).await;

    let input = server
        .client()
        .review_input(&repository(), number())
        .await
        .unwrap();

    let bodies: Vec<_> = input
        .context
        .comments
        .iter()
        .map(|thread| thread.comments[0].body.as_str())
        .collect();
    assert_eq!(
        bodies,
        ["Comment 100", "Commit comment 1", "Commit comment 2"]
    );
    assert_eq!(input.commits.len(), 2);
    let requests = server.requests();
    assert_eq!(requests.len(), 9);
    assert!(
        requests[6]
            .starts_with("GET /repos/contributor/fork/commits/abc1234/comments?per_page=100 ")
    );
    assert!(
        requests[7].starts_with("GET /repos/owner/repo/commits/def5678/comments?per_page=100 ")
    );
    assert!(requests[8].starts_with("GET /repos/owner/repo/pulls/123 "));
}

#[tokio::test]
async fn skips_a_forbidden_source_and_preserves_all_target_comments() {
    for message in [
        "Resource not accessible by integration",
        "Resource not accessible by personal access token",
    ] {
        assert_inaccessible_source_is_skipped(forbidden(message)).await;
    }
}

#[tokio::test]
async fn skips_a_source_requiring_sso_and_preserves_all_target_comments() {
    assert_inaccessible_source_is_skipped(
        failure(403).header("X-GitHub-SSO: required; url=https://github.com/orgs/example/sso"),
    )
    .await;
}

#[tokio::test]
async fn skips_a_missing_source_and_preserves_all_target_comments() {
    assert_inaccessible_source_is_skipped(failure(404)).await;
}

#[tokio::test]
async fn rejects_forbidden_target_comments() {
    let pull = pull_with_source(Some("contributor/fork"), "abc1234", 1);
    let mut replies = review_replies(&pull, &["abc1234"]);
    replies.push(forbidden("Resource not accessible by integration"));
    let server = Server::start(replies).await;

    assert_matches!(
        server.client().review_input(&repository(), number()).await,
        Err(GitHubError::Api { status: 403, .. })
    );
    assert_eq!(server.requests().len(), 6);
}

async fn initial_source_error(reply: Reply) -> GitHubError {
    let pull = pull_with_source(Some("contributor/fork"), "abc1234", 1);
    let mut replies = review_replies(&pull, &["abc1234"]);
    replies.extend([Reply::json(json!([commit_comment(1, "abc1234")])), reply]);
    let server = Server::start(replies).await;

    let error = server
        .client()
        .review_input(&repository(), number())
        .await
        .unwrap_err();
    assert_eq!(server.requests().len(), 7);
    error
}

#[tokio::test]
async fn rejects_initial_source_server_error() {
    assert_matches!(
        initial_source_error(failure(500)).await,
        GitHubError::Api { status: 500, .. }
    );
}

#[tokio::test]
async fn rejects_initial_source_forbidden_with_retry_after() {
    assert_matches!(
        initial_source_error(failure(403).header("Retry-After: 60")).await,
        GitHubError::Api {
            status: 403,
            rate_limited: true,
            ..
        }
    );
}

#[tokio::test]
async fn rejects_initial_source_secondary_rate_limit_without_rate_limit_headers() {
    assert_matches!(
        initial_source_error(forbidden(
            "You have exceeded a secondary rate limit. Please wait a few minutes before you try again."
        ))
        .await,
        GitHubError::Api {
            status: 403,
            rate_limited: true,
            ..
        }
    );
}

#[tokio::test]
async fn rejects_initial_source_secondary_rate_limit_even_when_sso_is_required() {
    assert_matches!(
        initial_source_error(
            forbidden("You have exceeded a secondary rate limit.")
                .header("X-GitHub-SSO: required; url=https://github.com/orgs/example/sso")
        )
        .await,
        GitHubError::Api {
            status: 403,
            rate_limited: true,
            ..
        }
    );
}

#[tokio::test]
async fn rejects_initial_source_forbidden_without_a_known_permission_error() {
    let mut malformed = failure(403);
    malformed.body = "{invalid json".to_string();
    for reply in [failure(403), forbidden("Forbidden"), malformed] {
        assert_matches!(
            initial_source_error(reply).await,
            GitHubError::Api {
                status: 403,
                rate_limited: false,
                permission_denied: false,
                ..
            }
        );
    }
}

#[tokio::test]
async fn rejects_a_forbidden_source_page_after_an_empty_page() {
    let pull = pull_with_source(Some("contributor/fork"), "abc1234", 1);
    let mut replies = review_replies(&pull, &["abc1234"]);
    replies.extend([
        Reply::json(json!([commit_comment(1, "abc1234")])),
        Reply::json(json!([])).header(concat!(
            "Link: <{base}repos/contributor/fork/commits/abc1234/comments?",
            "per_page=100&page=2>; rel=\"next\"",
        )),
        forbidden("Resource not accessible by integration"),
    ]);
    let server = Server::start(replies).await;

    assert_matches!(
        server.client().review_input(&repository(), number()).await,
        Err(GitHubError::Api { status: 403, .. })
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 8);
    assert!(requests[7].starts_with(concat!(
        "GET /repos/contributor/fork/commits/abc1234/comments?",
        "per_page=100&page=2 "
    )));
}

#[tokio::test]
async fn rejects_a_forbidden_source_commit_after_loading_no_comments() {
    let pull = pull_with_source(Some("contributor/fork"), "def5678", 2);
    let mut replies = review_replies(&pull, &["abc1234", "def5678"]);
    replies.extend([
        Reply::json(json!([commit_comment(1, "abc1234")])),
        Reply::json(json!([])),
        Reply::json(json!([commit_comment(3, "def5678")])),
        forbidden("Resource not accessible by integration"),
    ]);
    let server = Server::start(replies).await;

    let error = server
        .client()
        .review_input(&repository(), number())
        .await
        .unwrap_err();
    assert_matches!(error, GitHubError::Api { status: 403, .. });
    let requests = server.requests();
    assert_eq!(requests.len(), 9);
    assert!(
        requests[8]
            .starts_with("GET /repos/contributor/fork/commits/def5678/comments?per_page=100 ")
    );
}
