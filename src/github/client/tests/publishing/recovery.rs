use super::{inline::*, *};

fn failed(status: u16) -> Reply {
    let mut reply = created();
    reply.status = status;
    reply
}

fn known_comment(body: &str) -> Reply {
    Reply::json(json!([{
        "body": body,
        "html_url": "https://github.com/owner/repo/pull/123#confirmed"
    }]))
}

fn inline_findings(messages: &[&str]) -> RenderDocument {
    let findings = messages
        .iter()
        .map(|message| {
            json!({
                "commit": "abc1234",
                "severity": "high",
                "message": message,
                "file": "src/main.rs",
                "line": 5,
            })
        })
        .collect::<Vec<_>>();
    serde_json::from_value(json!({
        "ordered_commits": ["abc1234"],
        "stages": [],
        "findings": findings,
    }))
    .unwrap()
}

#[tokio::test]
async fn uncertain_inline_posts_are_reconciled_once_after_all_inline_posts() {
    let mut input = inline_findings(&[
        "Confirmed inline",
        "Unconfirmed inline",
        "Rejected inline",
        "Unpositioned issue",
        "Successful inline",
        "Confirmed inline without URL",
    ]);
    input.findings[3].location = None;
    let review = crate::github::feedback::PreparedReview::new(&input, &repository());
    let inline_body = |index: usize| {
        let item = &review.items[index];
        format!(
            "{}\n\n{}",
            item.body,
            crate::github::feedback::marker(&item.fingerprint)
        )
    };
    let body = inline_body(0);
    let body_without_url = inline_body(5);
    let mut replies = before_inline();
    replies.extend([
        failed(500),
        failed(502),
        failed(422),
        created(),
        failed(503),
        Reply::json(json!([
            {
                "body": body,
                "html_url": "https://github.com/owner/repo/pull/123#confirmed"
            },
            { "body": body_without_url }
        ])),
        created(),
    ]);
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();

    let requests = server.requests();
    assert_eq!(requests.len(), 12);
    assert!(requests[5].starts_with("POST /repos/owner/repo/pulls/123/comments "));
    assert!(requests[6].starts_with("POST /repos/owner/repo/pulls/123/comments "));
    assert!(requests[7].starts_with("POST /repos/owner/repo/pulls/123/comments "));
    assert!(requests[8].starts_with("POST /repos/owner/repo/pulls/123/comments "));
    assert!(requests[9].starts_with("POST /repos/owner/repo/pulls/123/comments "));
    assert!(requests[10].starts_with("GET /repos/owner/repo/pulls/123/comments?per_page=100 "));
    assert!(requests[11].starts_with("POST /repos/owner/repo/issues/123/comments "));

    assert!(
        request_body(&requests[5])["body"]
            .as_str()
            .unwrap()
            .contains("Confirmed inline")
    );
    assert!(
        request_body(&requests[6])["body"]
            .as_str()
            .unwrap()
            .contains("Unconfirmed inline")
    );
    assert!(
        request_body(&requests[7])["body"]
            .as_str()
            .unwrap()
            .contains("Rejected inline")
    );
    assert!(
        request_body(&requests[8])["body"]
            .as_str()
            .unwrap()
            .contains("Successful inline")
    );
    assert!(
        request_body(&requests[9])["body"]
            .as_str()
            .unwrap()
            .contains("Confirmed inline without URL")
    );

    let params = request_body(&requests[11]);
    let fallback = params["body"].as_str().unwrap();
    assert!(!fallback.contains("Confirmed inline"));
    assert!(!fallback.contains("Successful inline"));

    let unconfirmed = fallback.find("Unconfirmed inline").unwrap();
    let rejected = fallback.find("Rejected inline").unwrap();
    let unpositioned = fallback.find("Unpositioned issue").unwrap();
    assert!(unconfirmed < rejected);
    assert!(rejected < unpositioned);

    assert_eq!(crate::github::feedback::fingerprints(fallback).len(), 3);
    assert_eq!(report.published, 4);
    assert_eq!(report.recovered, 2);
    assert_eq!(report.inline, 3);
    assert_eq!(report.urls.len(), 3);
    assert!(
        report
            .to_string()
            .starts_with("Published 4 comment(s). 3 inline, 1 conversation.")
    );
    assert!(
        report
            .urls
            .iter()
            .any(|url| url == "https://github.com/owner/repo/pull/123#confirmed")
    );
}

#[tokio::test]
async fn an_inline_server_error_is_checked_before_posting_a_fallback() {
    let input = finding();
    let body = published_body(&input).await;
    let mut uncertain = failed(500);
    uncertain.body = "{}".into();
    let mut replies = before_inline();
    replies.extend([uncertain, known_comment(&body)]);
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.published, 1);
    assert_eq!(report.recovered, 1);
    assert_eq!(report.inline, 1);
    assert_eq!(
        report.urls,
        ["https://github.com/owner/repo/pull/123#confirmed"]
    );
    assert!(report.to_string().contains("Confirmed 1 comment(s)"));
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|request| request.starts_with("POST "))
            .count(),
        1
    );
}

#[tokio::test]
async fn an_uncertain_inline_post_with_a_missing_url_is_counted_after_confirmation() {
    let input = finding();
    let body = published_body(&input).await;
    let comment = json!({ "body": body });
    let mut replies = before_inline();
    replies.extend([failed(500), Reply::json(json!([comment]))]);
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();

    assert_eq!(report.published, 1);
    assert_eq!(report.inline, 1);
    assert_eq!(report.recovered, 1);
    assert_eq!(report.urls, Vec::<String>::new());
    assert_eq!(
        report.to_string(),
        concat!(
            "Published 1 comment(s). 1 inline, 0 conversation.",
            " Skipped 0 duplicate item(s) or summary.",
            " Confirmed 1 comment(s) after an uncertain response."
        )
    );
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|request| request.starts_with("POST "))
            .count(),
        1
    );
}

#[tokio::test]
async fn an_uncertain_inline_post_with_a_null_url_is_counted_after_confirmation() {
    let input = finding();
    let body = published_body(&input).await;
    let comment = json!({ "body": body, "html_url": null });
    let mut replies = before_inline();
    replies.extend([failed(500), Reply::json(json!([comment]))]);
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();

    assert_eq!(report.published, 1);
    assert_eq!(report.inline, 1);
    assert_eq!(report.recovered, 1);
    assert_eq!(report.urls, Vec::<String>::new());
    assert_eq!(
        report.to_string(),
        concat!(
            "Published 1 comment(s). 1 inline, 0 conversation.",
            " Skipped 0 duplicate item(s) or summary.",
            " Confirmed 1 comment(s) after an uncertain response."
        )
    );
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|request| request.starts_with("POST "))
            .count(),
        1
    );
}

#[tokio::test]
async fn an_invalid_inline_response_is_checked_before_posting_a_fallback() {
    let input = finding();
    let body = published_body(&input).await;
    let mut uncertain = failed(201);
    uncertain.body = "invalid JSON".into();
    let mut replies = before_inline();
    replies.extend([uncertain, known_comment(&body)]);
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.published, 1);
    assert_eq!(report.recovered, 1);
    assert_eq!(report.inline, 1);
    assert_eq!(
        report.urls,
        ["https://github.com/owner/repo/pull/123#confirmed"]
    );
    assert!(report.to_string().contains("Confirmed 1 comment(s)"));
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|request| request.starts_with("POST "))
            .count(),
        1
    );
}

#[tokio::test]
async fn a_timed_out_inline_post_is_confirmed_from_the_server() {
    let input = finding();
    let body = published_body(&input).await;
    let mut delayed = created();
    delayed.delay = Duration::from_millis(300);
    let mut replies = before_inline();
    replies.extend([delayed, known_comment(&body)]);
    let server = Server::start(replies).await;
    let client = GitHubClient::new(
        "test-token",
        server.base.clone(),
        Duration::from_millis(200),
    )
    .unwrap();
    let report = client
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.recovered, 1);
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|request| request.starts_with("POST "))
            .count(),
        1
    );
}

#[tokio::test]
async fn inline_recovery_checks_only_inline_comments_before_posting_a_fallback() {
    let mut replies = before_inline();
    replies.extend([failed(502), Reply::json(json!([])), created()]);
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &finding())
        .await
        .unwrap();
    let requests = server.requests();
    assert_eq!(requests.len(), 8);
    assert!(requests[1].starts_with("GET /repos/owner/repo/issues/123/comments?per_page=100 "));
    assert!(requests[2].starts_with("GET /repos/owner/repo/pulls/123/comments?per_page=100 "));
    assert!(requests[5].starts_with("POST /repos/owner/repo/pulls/123/comments "));
    assert!(requests[6].starts_with("GET /repos/owner/repo/pulls/123/comments?per_page=100 "));
    assert!(requests[7].starts_with("POST /repos/owner/repo/issues/123/comments "));
    assert_eq!(report.published, 1);
    assert_eq!(report.inline, 0);
    assert_eq!(report.recovered, 0);
    assert_eq!(report.urls.len(), 1);
}

#[tokio::test]
async fn inline_recovery_finds_a_matching_comment_on_a_later_page() {
    let input = finding();
    let body = published_body(&input).await;
    let page = Reply::json(json!([])).header(
        "Link: <{base}repos/owner/repo/pulls/123/comments?per_page=100&page=2>; rel=\"next\"",
    );
    let mut replies = before_inline();
    replies.extend([failed(500), page, known_comment(&body)]);
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();

    let requests = server.requests();
    assert_eq!(requests.len(), 8);
    assert!(requests[5].starts_with("POST /repos/owner/repo/pulls/123/comments "));
    assert!(requests[6].starts_with("GET /repos/owner/repo/pulls/123/comments?per_page=100 "));
    assert!(
        requests[7].starts_with("GET /repos/owner/repo/pulls/123/comments?per_page=100&page=2 ")
    );
    assert_eq!(report.published, 1);
    assert_eq!(report.inline, 1);
    assert_eq!(report.recovered, 1);
    assert_eq!(
        report.urls,
        ["https://github.com/owner/repo/pull/123#confirmed"]
    );
}

#[tokio::test]
async fn failed_reconciliation_stops_before_any_fallback() {
    let input = inline_findings(&["First uncertain", "Second uncertain", "Successful inline"]);
    let mut replies = before_inline();
    replies.extend([failed(500), failed(502), created(), failed(403)]);
    let server = Server::start(replies).await;
    assert_matches!(
        server
            .client()
            .publish(&repository(), number(), &input)
            .await,
        Err(GitHubError::Api { status: 403, .. })
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 9);
    assert!(requests[5].starts_with("POST /repos/owner/repo/pulls/123/comments "));
    assert!(requests[6].starts_with("POST /repos/owner/repo/pulls/123/comments "));
    assert!(requests[7].starts_with("POST /repos/owner/repo/pulls/123/comments "));
    assert!(requests[8].starts_with("GET /repos/owner/repo/pulls/123/comments?per_page=100 "));
}

#[tokio::test]
async fn an_uncertain_fallback_post_is_checked_with_fresh_feedback() {
    let input = finding();
    let body = published_body(&input).await;
    let mut replies = before_inline();
    replies.extend([
        failed(500),
        Reply::json(json!([])),
        failed(502),
        known_comment(&body),
        Reply::json(json!([])),
    ]);
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    let requests = server.requests();
    assert_eq!(requests.len(), 10);
    assert!(requests[5].starts_with("POST /repos/owner/repo/pulls/123/comments "));
    assert!(requests[6].starts_with("GET /repos/owner/repo/pulls/123/comments?per_page=100 "));
    assert!(requests[7].starts_with("POST /repos/owner/repo/issues/123/comments "));
    assert!(requests[8].starts_with("GET /repos/owner/repo/issues/123/comments?per_page=100 "));
    assert!(requests[9].starts_with("GET /repos/owner/repo/pulls/123/comments?per_page=100 "));
    assert_eq!(report.published, 1);
    assert_eq!(report.recovered, 1);
    assert_eq!(report.inline, 0);
    assert_eq!(
        report.urls,
        ["https://github.com/owner/repo/pull/123#confirmed"]
    );
}

#[tokio::test]
async fn an_uncertain_aggregate_post_with_every_marker_is_confirmed() {
    let input = document();
    let body = published_body(&input).await;
    let mut replies = before_publish();
    replies.extend([failed(500), known_comment(&body), Reply::json(json!([]))]);
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(crate::github::feedback::fingerprints(&body).len(), 3);
    assert_eq!(report.published, 1);
    assert_eq!(report.inline, 0);
    assert_eq!(report.recovered, 1);
    assert_eq!(report.urls.len(), 1);
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|request| request.starts_with("POST "))
            .count(),
        1
    );
}

#[tokio::test]
async fn an_uncertain_aggregate_post_with_a_missing_url_is_counted_once_after_confirmation() {
    let input = document();
    let body = published_body(&input).await;
    assert_eq!(crate::github::feedback::fingerprints(&body).len(), 3);
    let comment = json!({ "body": body });
    let mut replies = before_publish();
    replies.extend([
        failed(500),
        Reply::json(json!([comment])),
        Reply::json(json!([])),
    ]);
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();

    assert_eq!(report.published, 1);
    assert_eq!(report.inline, 0);
    assert_eq!(report.recovered, 1);
    assert_eq!(report.urls, Vec::<String>::new());
    assert_eq!(
        report.to_string(),
        concat!(
            "Published 1 comment(s). 0 inline, 1 conversation.",
            " Skipped 0 duplicate item(s) or summary.",
            " Confirmed 1 comment(s) after an uncertain response."
        )
    );
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|request| request.starts_with("POST "))
            .count(),
        1
    );
}

#[tokio::test]
async fn an_uncertain_aggregate_post_with_a_null_url_is_counted_once_after_confirmation() {
    let input = document();
    let body = published_body(&input).await;
    assert_eq!(crate::github::feedback::fingerprints(&body).len(), 3);
    let comment = json!({ "body": body, "html_url": null });
    let mut replies = before_publish();
    replies.extend([
        failed(500),
        Reply::json(json!([comment])),
        Reply::json(json!([])),
    ]);
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();

    assert_eq!(report.published, 1);
    assert_eq!(report.inline, 0);
    assert_eq!(report.recovered, 1);
    assert_eq!(report.urls, Vec::<String>::new());
    assert_eq!(
        report.to_string(),
        concat!(
            "Published 1 comment(s). 0 inline, 1 conversation.",
            " Skipped 0 duplicate item(s) or summary.",
            " Confirmed 1 comment(s) after an uncertain response."
        )
    );
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|request| request.starts_with("POST "))
            .count(),
        1
    );
}

#[tokio::test]
async fn an_uncertain_aggregate_post_without_markers_returns_the_original_error() {
    let input = document();
    let body = published_body(&input).await;
    let existing = body
        .lines()
        .filter(|line| !line.starts_with("<!--"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut replies = before_publish();
    replies.extend([
        failed(500),
        known_comment(&existing),
        Reply::json(json!([])),
    ]);
    let server = Server::start(replies).await;
    assert_matches!(
        server
            .client()
            .publish(&repository(), number(), &input)
            .await,
        Err(GitHubError::Api { status: 500, .. })
    );
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|request| request.starts_with("POST "))
            .count(),
        1
    );
}
