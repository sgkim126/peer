use super::{inline::*, *};

#[tokio::test]
async fn changed_base_prevents_publication() {
    let mut current: Value = serde_json::from_str(&pull().body).unwrap();
    current["base"]["sha"] = json!("fedcba9");
    let mut replies = before_inline();
    *replies.last_mut().unwrap() = Reply::json(current);
    let server = Server::start(replies).await;
    assert_matches!(
        server
            .client()
            .publish(&repository(), number(), &finding())
            .await,
        Err(GitHubError::PullRequestChanged)
    );
    assert_eq!(server.requests().len(), 7);
}

#[tokio::test]
async fn changed_head_prevents_publication() {
    let mut current: Value = serde_json::from_str(&pull().body).unwrap();
    current["head"]["sha"] = json!("fedcba9");
    let mut replies = before_inline();
    *replies.last_mut().unwrap() = Reply::json(current);
    let server = Server::start(replies).await;
    assert_matches!(
        server
            .client()
            .publish(&repository(), number(), &finding())
            .await,
        Err(GitHubError::PullRequestChanged)
    );
    assert_eq!(server.requests().len(), 7);
}

#[tokio::test]
async fn empty_files_with_changed_revision_prevent_publication() {
    for revision in ["base", "head"] {
        let mut current: Value = serde_json::from_str(&pull().body).unwrap();
        current[revision]["sha"] = json!("fedcba9");
        let mut replies = before_inline();
        replies[5] = Reply::json(json!([]));
        *replies.last_mut().unwrap() = Reply::json(current);
        let server = Server::start(replies).await;
        assert_matches!(
            server
                .client()
                .publish(&repository(), number(), &finding())
                .await,
            Err(GitHubError::PullRequestChanged)
        );
        let requests = server.requests();
        assert_eq!(requests.len(), 7);
        assert!(requests[6].starts_with("GET /repos/owner/repo/pulls/123 "));
        assert!(requests.iter().all(|request| request.starts_with("GET ")));
    }
}

#[tokio::test]
async fn failed_pr_revalidation_prevents_publication() {
    let mut replies = before_inline();
    replies.last_mut().unwrap().status = 500;
    let server = Server::start(replies).await;
    assert_matches!(
        server
            .client()
            .publish(&repository(), number(), &finding())
            .await,
        Err(GitHubError::Api { status: 500, .. })
    );
    assert_eq!(server.requests().len(), 7);
}

fn before_commit_fallback(files_failed: bool) -> (RenderDocument, Vec<Reply>) {
    let mut input = finding();
    let mut replies = vec![
        pull(),
        pr_commits(),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([])),
    ];
    if files_failed {
        let mut failure = Reply::json(json!({}));
        failure.status = 500;
        replies.push(failure);
    } else {
        input.findings[0].location = None;
    }
    (input, replies)
}

#[tokio::test]
async fn changed_revision_prevents_commit_fallback_without_pr_positions() {
    for files_failed in [false, true] {
        for revision in ["base", "head"] {
            let (input, mut replies) = before_commit_fallback(files_failed);
            let mut current: Value = serde_json::from_str(&pull().body).unwrap();
            current[revision]["sha"] = json!("fedcba9");
            replies.push(Reply::json(current));
            let expected_requests = replies.len();
            let server = Server::start(replies).await;

            assert_matches!(
                server
                    .client()
                    .publish(&repository(), number(), &input)
                    .await,
                Err(GitHubError::PullRequestChanged)
            );
            let requests = server.requests();
            assert_eq!(requests.len(), expected_requests);
            assert!(
                requests
                    .last()
                    .unwrap()
                    .starts_with("GET /repos/owner/repo/pulls/123 ")
            );
            assert!(posted_bodies(&server).is_empty());
        }
    }
}

#[tokio::test]
async fn failed_revalidation_prevents_commit_fallback_without_pr_positions() {
    for files_failed in [false, true] {
        let (input, mut replies) = before_commit_fallback(files_failed);
        let mut failure = pull();
        failure.status = 503;
        replies.push(failure);
        let expected_requests = replies.len();
        let server = Server::start(replies).await;

        assert_matches!(
            server
                .client()
                .publish(&repository(), number(), &input)
                .await,
            Err(GitHubError::Api { status: 503, .. })
        );
        let requests = server.requests();
        assert_eq!(requests.len(), expected_requests);
        assert!(
            requests
                .last()
                .unwrap()
                .starts_with("GET /repos/owner/repo/pulls/123 ")
        );
        assert!(posted_bodies(&server).is_empty());
    }
}

fn before_conversation_fallback_after_file_loading_failure() -> (RenderDocument, Vec<Reply>) {
    let (mut input, replies) = before_commit_fallback(true);
    input.findings[0].commit = CommitHash::new("fedcba9").unwrap();
    (input, replies)
}

#[tokio::test]
async fn changed_head_prevents_conversation_fallback_after_file_loading_failure() {
    let (input, mut replies) = before_conversation_fallback_after_file_loading_failure();
    let mut current: Value = serde_json::from_str(&pull().body).unwrap();
    current["head"]["sha"] = json!("def5678");
    replies.push(Reply::json(current));
    let server = Server::start(replies).await;

    assert_matches!(
        server
            .client()
            .publish(&repository(), number(), &input)
            .await,
        Err(GitHubError::PullRequestChanged)
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 7);
    assert!(requests[6].starts_with("GET /repos/owner/repo/pulls/123 "));
    assert_eq!(posted_bodies(&server), Vec::<String>::new());
}

#[tokio::test]
async fn changed_head_prevents_pending_summary_after_finding_deduplication() {
    let (mut input, mut replies) = before_commit_fallback(false);
    input.summary = Some(crate::review::ReviewSummary {
        peer_version: "0.16.2".into(),
    });
    let review = crate::github::feedback::PreparedReview::new(&input, &repository());
    replies[2] = Reply::json(json!([{
        "body": crate::github::feedback::marker(&review.items[0].fingerprint)
    }]));
    let mut current: Value = serde_json::from_str(&pull().body).unwrap();
    current["head"]["sha"] = json!("def5678");
    replies.push(Reply::json(current));
    let server = Server::start(replies).await;

    assert_matches!(
        server
            .client()
            .publish(&repository(), number(), &input)
            .await,
        Err(GitHubError::PullRequestChanged)
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 6);
    assert!(requests[5].starts_with("GET /repos/owner/repo/pulls/123 "));
    assert_eq!(posted_bodies(&server), Vec::<String>::new());
}

#[tokio::test]
async fn failed_revalidation_prevents_conversation_fallback_after_file_loading_failure() {
    let (input, mut replies) = before_conversation_fallback_after_file_loading_failure();
    let mut failure = pull();
    failure.status = 503;
    replies.push(failure);
    let server = Server::start(replies).await;

    assert_matches!(
        server
            .client()
            .publish(&repository(), number(), &input)
            .await,
        Err(GitHubError::Api { status: 503, .. })
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 7);
    assert!(requests[6].starts_with("GET /repos/owner/repo/pulls/123 "));
    assert_eq!(posted_bodies(&server), Vec::<String>::new());
}

#[tokio::test]
async fn unchanged_head_allows_conversation_fallback_after_file_loading_failure() {
    let (input, mut replies) = before_conversation_fallback_after_file_loading_failure();
    replies.extend([pull(), created()]);
    let server = Server::start(replies).await;

    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.published, 1);
    assert_eq!(report.inline, 0);
    assert_eq!(report.commit_comments, 0);
    let requests = server.requests();
    assert_eq!(requests.len(), 8);
    assert!(requests[6].starts_with("GET /repos/owner/repo/pulls/123 "));
    assert!(requests[7].starts_with("POST /repos/owner/repo/issues/123/comments "));
    assert_eq!(posted_bodies(&server).len(), 1);
}
