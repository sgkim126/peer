use super::*;

mod locations;

fn pull_with_source(source: Option<&str>, head: &str, commits: usize) -> Value {
    json!({
        "title": "Title", "body": "Description",
        "base": {"sha": "0123456"},
        "head": {"sha": head, "repo": source.map(|full_name| json!({"full_name": full_name}))},
        "commits": commits,
    })
}

#[tokio::test]
async fn loads_commit_comments_from_every_commit_and_repository_page() {
    let pull = pull_with_source(Some("contributor/fork"), "def5678", 2);
    let server = Server::start(vec![
        Reply::json(pull.clone()),
        Reply::json(json!([comment(100, Value::Null)])),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([{"sha": "abc1234"}, {"sha": "def5678"}])),
        Reply::json(json!([commit_comment(2, "abc1234")])).header(concat!(
            "Link: <{base}repos/owner/repo/commits/abc1234/comments?",
            "per_page=100&page=2>; rel=\"next\"",
        )),
        Reply::json(json!([commit_comment(1, "abc1234")])),
        Reply::json(json!([commit_comment(4, "abc1234")])).header(concat!(
            "Link: <{base}repos/contributor/fork/commits/abc1234/comments?",
            "per_page=100&page=2>; rel=\"next\"",
        )),
        Reply::json(json!([commit_comment(3, "abc1234")])),
        Reply::json(json!([commit_comment(5, "def5678")])),
        Reply::json(json!([commit_comment(6, "def5678")])),
        Reply::json(pull),
    ])
    .await;

    let input = server
        .client()
        .review_input(&repository(), number())
        .await
        .unwrap();

    let threads = input.context.comments;
    assert_eq!(threads.len(), 7);
    assert_eq!(threads[0].comments[0].body, "Comment 100");
    for (index, thread) in threads[1..].iter().enumerate() {
        assert_eq!(thread.comments.len(), 1);
        assert_eq!(
            thread.comments[0].body,
            format!("Commit comment {}", index + 1)
        );
        assert_eq!(
            thread.commit.as_ref().map(CommitHash::as_ref),
            Some(if index < 4 { "abc1234" } else { "def5678" })
        );
    }

    let requests = server.requests();
    assert_eq!(requests.len(), 12);
    for (index, path) in [
        (5, "repos/owner/repo/commits/abc1234/comments?per_page=100"),
        (
            6,
            "repos/owner/repo/commits/abc1234/comments?per_page=100&page=2",
        ),
        (
            7,
            "repos/contributor/fork/commits/abc1234/comments?per_page=100",
        ),
        (
            8,
            "repos/contributor/fork/commits/abc1234/comments?per_page=100&page=2",
        ),
        (9, "repos/owner/repo/commits/def5678/comments?per_page=100"),
        (
            10,
            "repos/contributor/fork/commits/def5678/comments?per_page=100",
        ),
        (11, "repos/owner/repo/pulls/123"),
    ] {
        assert!(requests[index].starts_with(&format!("GET /{path} ")));
    }
}

#[tokio::test]
async fn loads_only_target_comments_when_source_is_the_same_repository() {
    let pull = pull_with_source(Some("owner/repo"), "abc1234", 1);
    let server = Server::start(vec![
        Reply::json(pull.clone()),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([{"sha": "abc1234"}])),
        Reply::json(json!([commit_comment(1, "abc1234")])),
        Reply::json(pull),
    ])
    .await;

    let input = server
        .client()
        .review_input(&repository(), number())
        .await
        .unwrap();

    assert_eq!(input.context.comments.len(), 1);
    assert_eq!(
        input.context.comments[0].comments[0].body,
        "Commit comment 1"
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 7);
    assert!(
        requests[5].starts_with("GET /repos/owner/repo/commits/abc1234/comments?per_page=100 ")
    );
    assert!(requests[6].starts_with("GET /repos/owner/repo/pulls/123 "));
}

#[tokio::test]
async fn loads_only_target_comments_when_source_differs_only_in_case() {
    let pull = pull_with_source(Some("OWNER/REPO"), "abc1234", 1);
    let server = Server::start(vec![
        Reply::json(pull.clone()),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([{"sha": "abc1234"}])),
        Reply::json(json!([commit_comment(1, "abc1234")])),
        Reply::json(pull),
    ])
    .await;

    let input = server
        .client()
        .review_input(&repository(), number())
        .await
        .unwrap();

    assert_eq!(input.context.comments.len(), 1);
    assert_eq!(
        input.context.comments[0].comments[0].body,
        "Commit comment 1"
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 7);
    assert!(
        requests[5].starts_with("GET /repos/owner/repo/commits/abc1234/comments?per_page=100 ")
    );
    assert!(requests[6].starts_with("GET /repos/owner/repo/pulls/123 "));
}

#[tokio::test]
async fn loads_only_target_comments_when_source_repository_is_deleted() {
    let pull = pull_with_source(None, "abc1234", 1);
    let server = Server::start(vec![
        Reply::json(pull.clone()),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([{"sha": "abc1234"}])),
        Reply::json(json!([commit_comment(1, "abc1234")])),
        Reply::json(pull),
    ])
    .await;

    let input = server
        .client()
        .review_input(&repository(), number())
        .await
        .unwrap();

    assert_eq!(input.context.comments.len(), 1);
    assert_eq!(
        input.context.comments[0].comments[0].body,
        "Commit comment 1"
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 7);
    assert!(
        requests[5].starts_with("GET /repos/owner/repo/commits/abc1234/comments?per_page=100 ")
    );
    assert!(requests[6].starts_with("GET /repos/owner/repo/pulls/123 "));
}

#[tokio::test]
async fn a_missing_later_source_comment_page_discards_the_input() {
    let mut failure = Reply::json(json!({}));
    failure.status = 404;
    let server = Server::start(vec![
        Reply::json(pull_with_source(Some("contributor/fork"), "abc1234", 1)),
        Reply::json(json!([comment(100, Value::Null)])),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([{"sha": "abc1234"}])),
        Reply::json(json!([commit_comment(1, "abc1234")])),
        Reply::json(json!([commit_comment(2, "abc1234")])).header(concat!(
            "Link: <{base}repos/contributor/fork/commits/abc1234/comments?",
            "per_page=100&page=2>; rel=\"next\"",
        )),
        failure,
    ])
    .await;

    assert_matches!(
        server.client().review_input(&repository(), number()).await,
        Err(GitHubError::Api { status: 404, .. })
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 8);
    assert!(requests[7].starts_with(concat!(
        "GET /repos/contributor/fork/commits/abc1234/comments?",
        "per_page=100&page=2 "
    )));
}

#[tokio::test]
async fn rejects_changed_source_repository_after_loading_comments() {
    let server = Server::start(vec![
        Reply::json(pull_with_source(Some("contributor/fork"), "abc1234", 1)),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([{"sha": "abc1234"}])),
        Reply::json(json!([commit_comment(1, "abc1234")])),
        Reply::json(json!([commit_comment(2, "abc1234")])),
        Reply::json(pull_with_source(Some("other/fork"), "abc1234", 1)),
    ])
    .await;

    assert_matches!(
        server.client().review_input(&repository(), number()).await,
        Err(GitHubError::IncompleteCommits)
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 8);
    assert!(requests[7].starts_with("GET /repos/owner/repo/pulls/123 "));
}

#[tokio::test]
async fn rejects_deleted_source_repository_after_loading_comments() {
    let server = Server::start(vec![
        Reply::json(pull_with_source(Some("contributor/fork"), "abc1234", 1)),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([{"sha": "abc1234"}])),
        Reply::json(json!([commit_comment(1, "abc1234")])),
        Reply::json(json!([commit_comment(2, "abc1234")])),
        Reply::json(pull_with_source(None, "abc1234", 1)),
    ])
    .await;

    assert_matches!(
        server.client().review_input(&repository(), number()).await,
        Err(GitHubError::IncompleteCommits)
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 8);
    assert!(requests[7].starts_with("GET /repos/owner/repo/pulls/123 "));
}
