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
