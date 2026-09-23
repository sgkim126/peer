use super::*;
use crate::github::publish::CONVERSATION_MARKER;
use crate::render::RenderDocument;

mod file_fallback;
mod inline;
mod native;
mod recovery;
mod revalidation;
mod targets;

fn finding() -> RenderDocument {
    serde_json::from_value(json!({
        "ordered_commits": ["abc1234"],
        "stages": [],
        "findings": [{
            "commit": "abc1234",
            "severity": "high",
            "message": "Check @team <script>.",
            "file": "src/main.rs",
            "line": 5,
        }],
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
    let server = Server::start(vec![
        pull(),
        pr_commits(),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([])),
    ])
    .await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.urls, Vec::<String>::new());
    assert_eq!(report.published, 0);
    assert!(
        report
            .to_string()
            .starts_with("Published 0 comment(s). 0 inline, 0 commit, 0 conversation.")
    );
    assert_eq!(server.requests().len(), 5);
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

fn pr_commits() -> Reply {
    Reply::json(json!([{ "sha": "abc1234" }]))
}

fn before_publish() -> Vec<Reply> {
    vec![
        pull(),
        pr_commits(),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([])),
        pull(),
    ]
}

async fn published_body(input: &RenderDocument) -> String {
    published_bodies(input).await.join("\n\n")
}

async fn published_bodies(input: &RenderDocument) -> Vec<String> {
    let mut replies = before_publish();
    let review = crate::github::feedback::PreparedReview::new(input, &repository());
    replies.extend(review.items.iter().map(|_| created()));
    if review.summary_fingerprint.is_some() {
        replies.push(created());
    }
    let server = Server::start(replies).await;
    server
        .client()
        .publish(&repository(), number(), input)
        .await
        .unwrap();
    posted_bodies(&server)
}

#[tokio::test]
async fn publishes_rendered_input_to_the_selected_commit() {
    let url = "https://github.com/owner/repo/commit/abc1234#commitcomment-1";
    let mut reply = Reply::json(json!({ "html_url": url }));
    reply.status = 201;
    let mut replies = before_publish();
    replies.push(reply);
    let server = Server::start(replies).await;
    let input = finding();
    let review = crate::github::feedback::PreparedReview::new(&input, &repository());
    let item = &review.items[0];
    let body = format!(
        "{}\n\n{}",
        item.body,
        crate::github::feedback::marker(&item.fingerprint)
    );
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();

    let requests = server.requests();
    assert_eq!(requests.len(), 8);
    assert!(requests[0].starts_with("GET /repos/owner/repo/pulls/123 "));
    assert!(requests[6].starts_with("GET /repos/owner/repo/pulls/123 "));
    assert!(requests[7].starts_with("POST /repos/owner/repo/commits/abc1234/comments "));
    assert_eq!(request_body(&requests[7]), json!({ "body": body }));
    assert!(body.contains("**finding/high**"));
    assert_eq!(body.matches(CONVERSATION_MARKER).count(), 0);
    assert_eq!(crate::github::feedback::fingerprints(&body).len(), 1);
    assert_eq!(report.urls, [url]);
    assert_eq!(report.published, 1);
    assert_eq!(report.commit_comments, 1);
    assert!(
        report
            .to_string()
            .starts_with("Published 1 comment(s). 0 inline, 1 commit, 0 conversation.")
    );
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
    let mut input = finding();
    input.findings[0].commit = CommitHash::new("fedcba9").unwrap();
    let mut reply = created();
    reply.status = 403;
    let mut replies = before_publish();
    replies.push(reply);
    let server = Server::start(replies).await;
    assert_matches!(
        server
            .client()
            .publish(&repository(), number(), &input)
            .await,
        Err(GitHubError::Api { status: 403, .. })
    );
    assert_eq!(server.requests().len(), 8);
}

fn document() -> RenderDocument {
    serde_json::from_value(json!({
        "summary": {"peer_version": "0.14.0"},
        "ordered_commits": ["abc1234"],
        "findings": [
            {"commit": "abc1234", "severity": "high", "message": "First issue", "file": "src/main.rs", "line": 5},
            {"commit": "abc1234", "severity": "low", "message": "Second issue"}
        ],
        "stages": [{"stage": "quality", "target": "abc1234", "outcome": {"status": "clean", "summary": "Reviewed", "iterations": 1,
            "usage": [{
                "provider": "test",
                "model": "test",
                "input_tokens": 10,
                "output_tokens": 5,
                "cache_read_tokens": 0,
                "cache_write_tokens": 0,
                "cost_usd": 0.01
            }]
        }}],
    })).unwrap()
}

#[tokio::test]
async fn rerunning_skips_items_and_summary_despite_commit_and_usage_changes() {
    let mut input = document();
    let body = published_body(&input).await;
    assert_eq!(crate::github::feedback::fingerprints(&body).len(), 3);
    for finding in &mut input.findings {
        finding.commit = CommitHash::new("def5678").unwrap();
    }
    input.ordered_commits = vec![CommitHash::new("def5678").unwrap()];
    input.stages[0].target = crate::stage::StageTarget::Commit(CommitHash::new("def5678").unwrap());
    if let crate::render::RenderStageOutcome::Clean {
        usage, iterations, ..
    } = &mut input.stages[0].outcome
    {
        let mut model_usage = usage.iter().next().unwrap().clone();
        model_usage.model = "different-model".into();
        model_usage.cost_usd = 42.0;
        model_usage.input_tokens = 200;
        *usage = crate::llm::LlmUsage::from(vec![model_usage]);
        *iterations = 3;
    }
    let server = Server::start(vec![
        pull(),
        pr_commits(),
        Reply::json(json!([{ "body": body }])),
        Reply::json(json!([])),
        Reply::json(json!([])),
    ])
    .await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.urls, Vec::<String>::new());
    assert_eq!(report.published, 0);
    assert_eq!(report.skipped, 3);
    assert!(
        report
            .to_string()
            .starts_with("Published 0 comment(s). 0 inline, 0 commit, 0 conversation.")
    );
    assert_eq!(server.requests().len(), 5);
}

#[tokio::test]
async fn finds_duplicates_on_later_pages_of_conversation_comments() {
    let input = finding();
    let body = published_body(&input).await;
    let page = Reply::json(json!([])).header(
        "Link: <{base}repos/owner/repo/issues/123/comments?per_page=100&page=2>; rel=\"next\"",
    );
    let duplicate = Reply::json(json!([{ "body": body }]));
    let server = Server::start(vec![
        pull(),
        pr_commits(),
        page,
        duplicate,
        Reply::json(json!([])),
        Reply::json(json!([])),
    ])
    .await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.skipped, 1);
    assert_eq!(report.urls, Vec::<String>::new());
    assert_eq!(report.published, 0);
    assert_eq!(server.requests().len(), 6);
}

#[tokio::test]
async fn finds_duplicates_on_later_pages_of_inline_comments() {
    let input = finding();
    let body = published_body(&input).await;
    let page = Reply::json(json!([])).header(
        "Link: <{base}repos/owner/repo/pulls/123/comments?per_page=100&page=2>; rel=\"next\"",
    );
    let duplicate = Reply::json(json!([{ "body": body }]));
    let server = Server::start(vec![
        pull(),
        pr_commits(),
        Reply::json(json!([])),
        page,
        duplicate,
        Reply::json(json!([])),
    ])
    .await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.skipped, 1);
    assert_eq!(report.urls, Vec::<String>::new());
    assert_eq!(report.published, 0);
    assert_eq!(server.requests().len(), 6);
}

#[tokio::test]
async fn only_new_items_are_included_while_statistics_cover_the_full_review() {
    let mut input = document();
    let body = published_body(&input).await;
    input.findings[1].message = "Changed issue".into();
    let server = Server::start(vec![
        pull(),
        pr_commits(),
        Reply::json(json!([{ "body": body }])),
        Reply::json(json!([])),
        Reply::json(json!([])),
        pull(),
        created(),
        created(),
    ])
    .await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    let body = posted_bodies(&server).join("\n\n");
    assert!(!body.contains("First issue"));
    assert!(!body.contains("Second issue"));
    assert!(body.contains("Changed issue"));
    assert!(body.contains("**High findings:** 1"));
    assert!(body.contains("**Low findings:** 1"));
    assert_eq!(report.skipped, 1);
    assert_eq!(crate::github::feedback::fingerprints(&body).len(), 2);
}

#[tokio::test]
async fn duplicates_within_one_input_are_published_once() {
    let mut input = document();
    input.findings.push(input.findings[0].clone());
    let body = published_body(&input).await;
    assert_eq!(body.matches("First issue").count(), 1);
    assert!(body.contains("**High findings:** 2"));
}

#[tokio::test]
async fn failure_to_read_existing_conversation_comments_prevents_any_posts() {
    let mut failed = Reply::json(json!([]));
    failed.status = 500;
    let server = Server::start(vec![pull(), pr_commits(), failed]).await;
    assert_matches!(
        server
            .client()
            .publish(&repository(), number(), &finding())
            .await,
        Err(GitHubError::Api { status: 500, .. })
    );
    assert_eq!(server.requests().len(), 3);
}

#[tokio::test]
async fn failure_to_read_existing_inline_comments_prevents_any_posts() {
    let mut failed = Reply::json(json!([]));
    failed.status = 500;
    let server = Server::start(vec![pull(), pr_commits(), Reply::json(json!([])), failed]).await;
    assert_matches!(
        server
            .client()
            .publish(&repository(), number(), &finding())
            .await,
        Err(GitHubError::Api { status: 500, .. })
    );
    assert_eq!(server.requests().len(), 4);
}

#[tokio::test]
async fn incomplete_commit_list_prevents_feedback_lookup_and_publication() {
    let mut initial: Value = serde_json::from_str(&pull().body).unwrap();
    initial["commits"] = json!(2);
    let server = Server::start(vec![Reply::json(initial), pr_commits()]).await;
    assert_matches!(
        server
            .client()
            .publish(&repository(), number(), &finding())
            .await,
        Err(GitHubError::IncompleteCommits)
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].starts_with("GET /repos/owner/repo/pulls/123 "));
    assert!(requests[1].starts_with("GET /repos/owner/repo/pulls/123/commits?per_page=100 "));
}

#[tokio::test]
async fn mismatched_commit_list_head_prevents_feedback_lookup_and_publication() {
    let server = Server::start(vec![pull(), Reply::json(json!([{ "sha": "def5678" }]))]).await;
    assert_matches!(
        server
            .client()
            .publish(&repository(), number(), &finding())
            .await,
        Err(GitHubError::IncompleteCommits)
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].starts_with("GET /repos/owner/repo/pulls/123 "));
    assert!(requests[1].starts_with("GET /repos/owner/repo/pulls/123/commits?per_page=100 "));
}

#[tokio::test]
async fn duplicate_commits_prevent_feedback_lookup_and_publication() {
    let mut initial: Value = serde_json::from_str(&pull().body).unwrap();
    initial["commits"] = json!(2);
    let server = Server::start(vec![
        Reply::json(initial),
        Reply::json(json!([{ "sha": "abc1234" }, { "sha": "abc1234" }])),
    ])
    .await;
    assert_matches!(
        server
            .client()
            .publish(&repository(), number(), &finding())
            .await,
        Err(GitHubError::IncompleteCommits)
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].starts_with("GET /repos/owner/repo/pulls/123 "));
    assert!(requests[1].starts_with("GET /repos/owner/repo/pulls/123/commits?per_page=100 "));
}

fn posted_bodies(server: &Server) -> Vec<String> {
    server
        .requests()
        .iter()
        .filter(|request| request.starts_with("POST "))
        .map(|request| request_body(request)["body"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn rerunning_after_summary_failure_posts_only_the_summary() {
    let input = document();
    let mut replies = before_publish();
    let mut rejected = created();
    rejected.status = 403;
    replies.extend([created(), created(), rejected]);
    let first = Server::start(replies).await;
    assert!(
        first
            .client()
            .publish(&repository(), number(), &input)
            .await
            .is_err()
    );
    let bodies = posted_bodies(&first);
    assert_eq!(bodies.len(), 3);
    assert!(!bodies[0].contains(CONVERSATION_MARKER));
    assert!(bodies[2].contains(CONVERSATION_MARKER));
    assert!(!bodies[2].contains("First issue"));
    let second = Server::start(vec![
        pull(),
        pr_commits(),
        Reply::json(json!([{"body": bodies[0]}, {"body": bodies[1]}])),
        Reply::json(json!([])),
        Reply::json(json!([])),
        created(),
    ])
    .await;
    let report = second
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.skipped, 2);
    assert_eq!(report.published, 1);
    assert_eq!(posted_bodies(&second), vec![bodies[2].clone()]);
}

#[tokio::test]
async fn an_existing_summary_does_not_suppress_new_feedback() {
    let input = document();
    let bodies = published_bodies(&input).await;
    let mut replies = before_publish();
    replies[2] = Reply::json(json!([{"body": bodies[2]}]));
    replies.extend([created(), created()]);
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.skipped, 1);
    assert_eq!(posted_bodies(&server), bodies[..2].to_vec());
}

#[tokio::test]
async fn a_summary_only_review_creates_one_marked_comment() {
    let mut input = document();
    input.findings.clear();
    let server = Server::start(vec![
        pull(),
        pr_commits(),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([])),
        created(),
    ])
    .await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    let bodies = posted_bodies(&server);
    assert_eq!(report.published, 1);
    assert_eq!(bodies.len(), 1);
    assert!(bodies[0].contains(CONVERSATION_MARKER));
    assert_eq!(crate::github::feedback::fingerprints(&bodies[0]).len(), 1);
}

#[tokio::test]
async fn individual_feedback_retries_only_the_unpublished_items() {
    let mut input = document();
    for finding in &mut input.findings {
        finding.commit = CommitHash::new("fedcba9").unwrap();
    }
    let mut replies = before_publish();
    let mut rejected = created();
    rejected.status = 403;
    replies.extend([created(), rejected]);
    let first = Server::start(replies).await;
    assert!(
        first
            .client()
            .publish(&repository(), number(), &input)
            .await
            .is_err()
    );
    let bodies = posted_bodies(&first);
    assert_eq!(bodies.len(), 2);
    assert!(
        bodies
            .iter()
            .all(|body| crate::github::feedback::fingerprints(body).len() == 1)
    );
    assert!(bodies[0].contains("First issue"));
    assert!(bodies[1].contains("Second issue"));
    let second = Server::start(vec![
        pull(),
        pr_commits(),
        Reply::json(json!([{"body": bodies[0]}])),
        Reply::json(json!([])),
        Reply::json(json!([])),
        created(),
        created(),
    ])
    .await;
    let report = second
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    let retried = posted_bodies(&second);
    assert_eq!(report.skipped, 1);
    assert_eq!(report.published, 2);
    assert_eq!(retried[0], bodies[1]);
    assert!(retried[1].contains(CONVERSATION_MARKER));
}

#[tokio::test]
async fn finds_old_aggregate_markers_on_a_different_current_commit() {
    let input = document();
    let review = crate::github::feedback::PreparedReview::new(&input, &repository());
    let body = review.aggregate(&review.items.iter().collect::<Vec<_>>(), true);
    let mut initial: Value = serde_json::from_str(&pull().body).unwrap();
    initial["commits"] = json!(2);
    let server = Server::start(vec![
        Reply::json(initial),
        Reply::json(json!([{"sha": "def5678"}])).header(
            "Link: <{base}repos/owner/repo/pulls/123/commits?per_page=100&page=2>; rel=\"next\"",
        ),
        pr_commits(),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([])).header(
            "Link: <{base}repos/owner/repo/commits/def5678/comments?per_page=100&page=2>; rel=\"next\"",
        ),
        Reply::json(json!([{"body": body}])),
        Reply::json(json!([])),
    ])
    .await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.published, 0);
    assert_eq!(report.skipped, 3);
    let requests = server.requests();
    assert_eq!(requests.len(), 8);
    assert!(requests[1].starts_with("GET /repos/owner/repo/pulls/123/commits?per_page=100 "));
    assert!(
        requests[5].starts_with("GET /repos/owner/repo/commits/def5678/comments?per_page=100 ")
    );
    assert!(
        requests[6]
            .starts_with("GET /repos/owner/repo/commits/def5678/comments?per_page=100&page=2 ")
    );
    assert!(
        requests[7].starts_with("GET /repos/owner/repo/commits/abc1234/comments?per_page=100 ")
    );
    for request in requests.iter() {
        assert!(request.starts_with("GET "));
    }
}
