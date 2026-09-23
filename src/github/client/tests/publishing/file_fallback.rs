use super::{inline::before_inline, *};

fn rejected(status: u16) -> Reply {
    let mut reply = Reply::json(json!({}));
    reply.status = status;
    reply
}

fn confirmed(input: &RenderDocument) -> Reply {
    let review = crate::github::feedback::PreparedReview::new(input, &repository());
    Reply::json(json!([{
        "body": crate::github::feedback::marker(&review.items[0].fingerprint),
        "html_url": "https://github.com/owner/repo/pull/123#discussion_r42"
    }]))
}

#[tokio::test]
async fn missing_patches_fall_back_to_pr_file_comments() {
    let mut replies = before_inline();
    replies[4] = Reply::json(json!([{
        "filename": "src/main.rs"
    }]));
    replies.push(created());
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &finding())
        .await
        .unwrap();
    let requests = server.requests();
    let post = requests.last().unwrap();
    assert!(post.starts_with("POST /repos/owner/repo/pulls/123/comments "));
    let body = request_body(post);
    assert_eq!(body["subject_type"], "file");
    assert_eq!(body["path"], "src/main.rs");
    assert_eq!(body["commit_id"], "abc1234");
    assert!(body.get("line").is_none());
    assert_eq!(report.inline, 1);
    assert_eq!(posted_bodies(&server).len(), 1);
}

#[tokio::test]
async fn old_paths_after_rename_fall_back_to_pr_file_comments_on_the_new_path() {
    let mut replies = before_inline();
    replies[4] = Reply::json(json!([{
        "filename": "src/new.rs",
        "previous_filename": "src/main.rs",
        "patch": "@@ -5 +5 @@\n-old\n+new"
    }]));
    replies.push(created());
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &finding())
        .await
        .unwrap();
    let requests = server.requests();
    let post = requests.last().unwrap();
    assert!(post.starts_with("POST /repos/owner/repo/pulls/123/comments "));
    let body = request_body(post);
    assert_eq!(body["subject_type"], "file");
    assert_eq!(body["path"], "src/new.rs");
    assert_eq!(body["commit_id"], "abc1234");
    assert!(body.get("line").is_none());
    assert_eq!(report.inline, 1);
    assert_eq!(posted_bodies(&server).len(), 1);
}

#[tokio::test]
async fn rejected_line_comments_fall_back_to_file_comments() {
    let mut replies = before_inline();
    replies.extend([rejected(422), created()]);
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &finding())
        .await
        .unwrap();
    let requests = server.requests();
    assert_eq!(request_body(&requests[6])["line"], 5);
    assert_eq!(request_body(&requests[7])["subject_type"], "file");
    assert_eq!(posted_bodies(&server)[0], posted_bodies(&server)[1]);
    assert_eq!(report.published, 1);
    assert_eq!(report.inline, 1);
}

#[tokio::test]
async fn uncertain_line_is_checked_before_the_file_fallback() {
    let mut replies = before_inline();
    replies.extend([rejected(502), Reply::json(json!([])), created()]);
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &finding())
        .await
        .unwrap();
    let requests = server.requests();
    assert!(requests[7].starts_with("GET /repos/owner/repo/pulls/123/comments?"));
    assert_eq!(request_body(&requests[8])["subject_type"], "file");
    assert_eq!(report.inline, 1);
    assert_eq!(report.recovered, 0);
}

#[tokio::test]
async fn confirmed_file_fallback_does_not_create_a_conversation_comment() {
    let input = finding();
    let mut replies = before_inline();
    replies.extend([rejected(422), rejected(502), confirmed(&input)]);
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    let requests = server.requests();
    assert_eq!(requests.len(), 9);
    assert_eq!(request_body(&requests[7])["subject_type"], "file");
    assert!(requests[8].starts_with("GET /repos/owner/repo/pulls/123/comments?"));
    assert_eq!(report.inline, 1);
    assert_eq!(report.recovered, 1);
    assert_eq!(report.published, 1);
}

#[tokio::test]
async fn failed_file_confirmation_stops_before_further_fallbacks() {
    let mut replies = before_inline();
    replies.extend([rejected(422), rejected(502), rejected(403)]);
    let server = Server::start(replies).await;
    assert_matches!(
        server
            .client()
            .publish(&repository(), number(), &finding())
            .await,
        Err(GitHubError::Api { status: 403, .. })
    );
    assert_eq!(server.requests().len(), 9);
    assert_eq!(posted_bodies(&server).len(), 2);
}
