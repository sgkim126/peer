use super::*;
use crate::render::{RenderInput, github};

mod inline;
mod recovery;
mod revalidation;

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
    let server = Server::start(vec![pull(), Reply::json(json!([])), Reply::json(json!([]))]).await;
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
            .starts_with("Published 0 comment(s). 0 inline, 0 conversation.")
    );
    assert_eq!(server.requests().len(), 3);
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

fn before_publish() -> Vec<Reply> {
    vec![
        pull(),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([])),
        pull(),
    ]
}

async fn published_body(input: &RenderInput) -> String {
    let mut replies = before_publish();
    replies.push(created());
    let server = Server::start(replies).await;
    server
        .client()
        .publish(&repository(), number(), input)
        .await
        .unwrap();
    request_body(server.requests().last().unwrap())["body"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn publishes_rendered_input_to_the_selected_pull_request() {
    let mut replies = before_publish();
    replies.push(created());
    let server = Server::start(replies).await;
    let input = finding();
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();

    let requests = server.requests();
    assert_eq!(requests.len(), 6);
    assert!(requests[0].starts_with("GET /repos/owner/repo/pulls/123 "));
    assert!(requests[4].starts_with("GET /repos/owner/repo/pulls/123 "));
    assert!(requests[5].starts_with("POST /repos/owner/repo/issues/123/comments "));
    let body = request_body(&requests[5])["body"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(body.starts_with(&github::render(&input, "owner/repo")));
    assert_eq!(crate::github::feedback::fingerprints(&body).len(), 1);
    assert_eq!(report.urls.len(), 1);
    assert_eq!(report.published, 1);
    assert!(
        report
            .to_string()
            .starts_with("Published 1 comment(s). 0 inline, 1 conversation.")
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
    let mut reply = created();
    reply.status = 403;
    let mut replies = before_publish();
    replies.push(reply);
    let server = Server::start(replies).await;
    assert_matches!(
        server
            .client()
            .publish(&repository(), number(), &finding())
            .await,
        Err(GitHubError::Api { status: 403, .. })
    );
    assert_eq!(server.requests().len(), 6);
}

fn document() -> RenderInput {
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
    if let RenderInput::Document(document) = &mut input {
        for finding in &mut document.findings {
            finding.commit = CommitHash::new("def5678").unwrap();
        }
        document.ordered_commits = vec![CommitHash::new("def5678").unwrap()];
        document.stages[0].target =
            crate::stage::StageTarget::Commit(CommitHash::new("def5678").unwrap());
        if let crate::render::RenderStageOutcome::Clean {
            usage, iterations, ..
        } = &mut document.stages[0].outcome
        {
            let mut model_usage = usage.iter().next().unwrap().clone();
            model_usage.model = "different-model".into();
            model_usage.cost_usd = 42.0;
            model_usage.input_tokens = 200;
            *usage = crate::llm::LlmUsage::from(vec![model_usage]);
            *iterations = 3;
        }
    }
    let server = Server::start(vec![
        pull(),
        Reply::json(json!([{ "body": body }])),
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
            .starts_with("Published 0 comment(s). 0 inline, 0 conversation.")
    );
    assert_eq!(server.requests().len(), 3);
}

#[tokio::test]
async fn finds_duplicates_on_later_pages_of_conversation_comments() {
    let input = finding();
    let body = published_body(&input).await;
    let page = Reply::json(json!([])).header(
        "Link: <{base}repos/owner/repo/issues/123/comments?per_page=100&page=2>; rel=\"next\"",
    );
    let duplicate = Reply::json(json!([{ "body": body }]));
    let server = Server::start(vec![pull(), page, duplicate, Reply::json(json!([]))]).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.skipped, 1);
    assert_eq!(report.urls, Vec::<String>::new());
    assert_eq!(report.published, 0);
    assert_eq!(server.requests().len(), 4);
}

#[tokio::test]
async fn finds_duplicates_on_later_pages_of_inline_comments() {
    let input = finding();
    let body = published_body(&input).await;
    let page = Reply::json(json!([])).header(
        "Link: <{base}repos/owner/repo/pulls/123/comments?per_page=100&page=2>; rel=\"next\"",
    );
    let duplicate = Reply::json(json!([{ "body": body }]));
    let server = Server::start(vec![pull(), Reply::json(json!([])), page, duplicate]).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.skipped, 1);
    assert_eq!(report.urls, Vec::<String>::new());
    assert_eq!(report.published, 0);
    assert_eq!(server.requests().len(), 4);
}

#[tokio::test]
async fn only_new_items_are_included_while_statistics_cover_the_full_review() {
    let mut input = document();
    let body = published_body(&input).await;
    if let RenderInput::Document(document) = &mut input {
        document.findings[1].message = "Changed issue".into();
    }
    let server = Server::start(vec![
        pull(),
        Reply::json(json!([{ "body": body }])),
        Reply::json(json!([])),
        created(),
    ])
    .await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    let body = request_body(server.requests().last().unwrap())["body"]
        .as_str()
        .unwrap()
        .to_string();
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
    if let RenderInput::Document(document) = &mut input {
        document.findings.push(document.findings[0].clone());
    }
    let body = published_body(&input).await;
    assert_eq!(body.matches("First issue").count(), 1);
    assert!(body.contains("**High findings:** 2"));
}

#[tokio::test]
async fn failure_to_read_existing_conversation_comments_prevents_any_posts() {
    let mut failed = Reply::json(json!([]));
    failed.status = 500;
    let server = Server::start(vec![pull(), failed]).await;
    assert_matches!(
        server
            .client()
            .publish(&repository(), number(), &finding())
            .await,
        Err(GitHubError::Api { status: 500, .. })
    );
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn failure_to_read_existing_inline_comments_prevents_any_posts() {
    let mut failed = Reply::json(json!([]));
    failed.status = 500;
    let server = Server::start(vec![pull(), Reply::json(json!([])), failed]).await;
    assert_matches!(
        server
            .client()
            .publish(&repository(), number(), &finding())
            .await,
        Err(GitHubError::Api { status: 500, .. })
    );
    assert_eq!(server.requests().len(), 3);
}
