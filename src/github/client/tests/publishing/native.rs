use super::*;

fn failed(status: u16) -> Reply {
    let mut reply = created();
    reply.status = status;
    reply
}

fn commit_finding() -> RenderDocument {
    let mut input = finding();
    input.findings[0].location = None;
    input
}

fn before_commit() -> Vec<Reply> {
    vec![
        pull(),
        pr_commits(),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([])),
        pull(),
    ]
}

fn native_created() -> Reply {
    let mut reply = Reply::json(json!({
        "html_url": "https://github.com/owner/repo/commit/abc1234#commitcomment-1"
    }));
    reply.status = 201;
    reply
}

fn native_body(input: &RenderDocument) -> String {
    let review = crate::github::feedback::PreparedReview::new(input, &repository());
    let item = &review.items[0];
    format!(
        "{}\n\n{}",
        item.body,
        crate::github::feedback::marker(&item.fingerprint)
    )
}

#[tokio::test]
async fn rejected_commit_comments_fall_back_to_individual_conversation_comments() {
    let input = commit_finding();
    let mut replies = before_commit();
    replies.extend([failed(403), created()]);
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    let requests = server.requests();
    assert_eq!(requests.len(), 8);
    assert!(requests[5].starts_with("GET /repos/owner/repo/pulls/123 "));
    assert!(requests[6].starts_with("POST /repos/owner/repo/commits/abc1234/comments "));
    assert!(requests[7].starts_with("POST /repos/owner/repo/issues/123/comments "));
    assert_eq!(request_body(&requests[6]), request_body(&requests[7]));
    assert_eq!(report.published, 1);
    assert_eq!(report.commit_comments, 0);
    assert_eq!(report.recovered, 0);
}

#[tokio::test]
async fn uncertain_native_posts_are_confirmed_on_later_pages_of_the_same_commit() {
    let input = commit_finding();
    let body = native_body(&input);
    let mut replies = before_commit();
    replies.extend([
        failed(500),
        Reply::json(json!([])).header(
            "Link: <{base}repos/owner/repo/commits/abc1234/comments?per_page=100&page=2>; rel=\"next\"",
        ),
        Reply::json(json!([{"body": body, "html_url": "https://github.com/owner/repo/commit/abc1234#commitcomment-1"}])),
    ]);
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.published, 1);
    assert_eq!(report.commit_comments, 1);
    assert_eq!(report.recovered, 1);
    assert_eq!(report.urls.len(), 1);
    let requests = server.requests();
    assert_eq!(requests.len(), 9);
    assert!(
        requests[7].starts_with("GET /repos/owner/repo/commits/abc1234/comments?per_page=100 ")
    );
    assert!(
        requests[8]
            .starts_with("GET /repos/owner/repo/commits/abc1234/comments?per_page=100&page=2 ")
    );
    assert_eq!(posted_bodies(&server).len(), 1);
}

#[tokio::test]
async fn a_confirmed_native_comment_without_a_url_is_counted_once() {
    let input = commit_finding();
    let mut replies = before_commit();
    replies.extend([
        failed(500),
        Reply::json(json!([{"body": native_body(&input)}])),
    ]);
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.published, 1);
    assert_eq!(report.commit_comments, 1);
    assert_eq!(report.recovered, 1);
    assert!(report.urls.is_empty());
    assert_eq!(posted_bodies(&server).len(), 1);
}

#[tokio::test]
async fn unconfirmed_native_posts_fall_back_without_repeating_the_native_post() {
    let input = commit_finding();
    let mut replies = before_commit();
    replies.extend([failed(500), Reply::json(json!([])), created()]);
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    let requests = server.requests();
    assert_eq!(requests.len(), 9);
    assert!(requests[8].starts_with("POST /repos/owner/repo/issues/123/comments "));
    assert_eq!(
        posted_bodies(&server),
        [native_body(&input), native_body(&input)]
    );
    assert_eq!(report.published, 1);
    assert_eq!(report.commit_comments, 0);
    assert_eq!(report.recovered, 0);
}

#[tokio::test]
async fn failed_native_confirmation_stops_before_any_conversation_post() {
    let mut replies = before_commit();
    replies.extend([failed(500), failed(503)]);
    let server = Server::start(replies).await;
    assert_matches!(
        server
            .client()
            .publish(&repository(), number(), &commit_finding())
            .await,
        Err(GitHubError::Api { status: 503, .. })
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 8);
    assert!(
        requests[7].starts_with("GET /repos/owner/repo/commits/abc1234/comments?per_page=100 ")
    );
    assert_eq!(posted_bodies(&server).len(), 1);
}

#[tokio::test]
async fn rerunning_partial_native_publication_posts_only_missing_feedback_and_summary() {
    let input = document();
    let mut replies = before_publish();
    replies.extend([native_created(), failed(403), failed(403)]);
    let first = Server::start(replies).await;
    assert_matches!(
        first
            .client()
            .publish(&repository(), number(), &input)
            .await,
        Err(GitHubError::Api { status: 403, .. })
    );
    let posted = posted_bodies(&first);
    assert_eq!(posted.len(), 3);
    assert_eq!(posted[1], posted[2]);
    let mut replies = before_commit();
    replies[4] = Reply::json(json!([{"body": posted[0]}]));
    replies.extend([native_created(), created()]);
    let second = Server::start(replies).await;
    let report = second
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.skipped, 1);
    assert_eq!(report.published, 2);
    assert_eq!(report.commit_comments, 1);
    assert_eq!(report.inline, 0);
    assert!(
        report
            .to_string()
            .starts_with("Published 2 comment(s). 0 inline, 1 commit, 1 conversation.")
    );
    let posted = posted_bodies(&second);
    assert!(posted[0].contains("Second issue"));
    assert!(!posted[0].contains(CONVERSATION_MARKER));
    assert!(posted[1].contains(CONVERSATION_MARKER));
    assert!(!posted[1].contains("Second issue"));
}

#[tokio::test]
async fn questions_and_recommendations_without_related_commits_use_conversation_comments() {
    let input = serde_json::from_value(json!({
        "ordered_commits": [], "stages": [],
        "questions": [{
            "category": "rationale",
            "question": "Why?",
            "evidence": "Evidence",
            "why_it_matters": "Reason",
            "related_commits": []
        }],
        "recommendations": [{
            "kind": "split_commit",
            "message": "Split this",
            "rationale": "Reason",
            "related_commits": []
        }],
    }))
    .unwrap();
    let mut replies = before_commit();
    replies.pop();
    replies.extend([created(), created()]);
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    let requests = server.requests();
    assert_eq!(requests.len(), 7);
    assert!(
        requests[5..]
            .iter()
            .all(|request| request.starts_with("POST /repos/owner/repo/issues/123/comments "))
    );
    assert_eq!(report.published, 2);
    assert_eq!(report.commit_comments, 0);
    for body in posted_bodies(&server) {
        assert_eq!(crate::github::feedback::fingerprints(&body).len(), 1);
    }
}
