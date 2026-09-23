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
