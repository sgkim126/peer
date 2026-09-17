use super::*;

fn file_finding() -> RenderInput {
    serde_json::from_value(json!({
        "ordered_commits": ["abc1234"],
        "stages": [],
        "findings": [{
            "commit": "abc1234",
            "severity": "high",
            "message": "Check @team <script>.",
            "file": "src/main.rs"
        }],
    }))
    .unwrap()
}

fn file_document() -> RenderInput {
    let mut input = document();
    let RenderInput::Document(document) = &mut input;
    for finding in &mut document.findings {
        if let Some(location) = &mut finding.location {
            location.line = None;
        }
    }
    input
}

fn changed_file() -> Reply {
    Reply::json(json!([{
        "filename": "src/main.rs",
        "patch": "@@ -4,3 +4,3 @@\n context\n-old\n+new\n context"
    }]))
}

pub fn before_inline() -> Vec<Reply> {
    vec![
        pull(),
        Reply::json(json!([])),
        Reply::json(json!([])),
        changed_file(),
        pull(),
    ]
}

#[tokio::test]
async fn publishes_file_comment_at_the_current_head_with_a_hidden_fingerprint() {
    let mut replies = before_inline();
    replies.push(created());
    let server = Server::start(replies).await;
    let mut input = file_finding();
    let RenderInput::Document(document) = &mut input;
    document.findings[0].commit = CommitHash::new("def5678").unwrap();
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    let requests = server.requests();
    let request = requests.last().unwrap();
    assert!(request.starts_with("POST /repos/owner/repo/pulls/123/comments "));
    let params = request_body(request);
    assert_eq!(params["commit_id"], "abc1234");
    assert_eq!(params["path"], "src/main.rs");
    assert_eq!(params["subject_type"], "file");
    assert!(params.get("line").is_none());
    assert!(params.get("side").is_none());
    assert!(
        !params["body"]
            .as_str()
            .unwrap()
            .contains(CONVERSATION_MARKER)
    );
    assert_eq!(
        crate::github::feedback::fingerprints(params["body"].as_str().unwrap()).len(),
        1
    );
    assert_eq!(report.inline, 1);
    assert_eq!(report.urls.len(), 1);
    assert_eq!(report.published, 1);
    assert!(
        report
            .to_string()
            .starts_with("Published 1 comment(s). 1 inline, 0 conversation.")
    );
    assert_eq!(requests.len(), 6);
}

#[tokio::test]
async fn combines_inline_failures_with_unpositioned_items_and_summary() {
    let mut replies = before_inline();
    let mut rejected = created();
    rejected.status = 422;
    replies.extend([rejected, created()]);
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &file_document())
        .await
        .unwrap();
    let requests = server.requests();
    assert!(requests[5].starts_with("POST /repos/owner/repo/pulls/123/comments "));
    assert!(requests[6].starts_with("POST /repos/owner/repo/issues/123/comments "));
    let body = request_body(&requests[6])["body"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(body.contains("First issue"));
    assert!(body.contains("Second issue"));
    assert!(body.contains("## Review summary"));
    assert!(body.contains("<summary>Stage:"));
    assert_eq!(body.matches(CONVERSATION_MARKER).count(), 1);
    assert_eq!(crate::github::feedback::fingerprints(&body).len(), 3);
    assert_eq!(report.inline, 0);
    assert_eq!(report.urls.len(), 1);
    assert_eq!(report.published, 1);
    assert!(
        report
            .to_string()
            .starts_with("Published 1 comment(s). 0 inline, 1 conversation.")
    );
}

#[tokio::test]
async fn keeps_summary_and_full_counts_even_when_every_item_is_inline() {
    let mut input = file_document();
    let RenderInput::Document(document) = &mut input;
    document.findings.pop();
    let mut replies = before_inline();
    replies.extend([created(), created()]);
    let server = Server::start(replies).await;
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
    assert!(!body.contains("## Review findings"));
    assert!(body.contains("**High findings:** 1"));
    assert!(body.contains("<summary>Stage:"));
    assert_eq!(body.matches(CONVERSATION_MARKER).count(), 1);
    assert_eq!(crate::github::feedback::fingerprints(&body).len(), 1);
    assert_eq!(report.inline, 1);
    assert_eq!(report.urls.len(), 2);
    assert_eq!(report.published, 2);
    assert!(
        report
            .to_string()
            .starts_with("Published 2 comment(s). 1 inline, 1 conversation.")
    );
}

#[tokio::test]
async fn failure_to_load_files_falls_back_to_a_conversation_comment() {
    let mut replies = before_publish();
    replies.pop();
    replies[3].status = 500;
    replies.push(created());
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &file_finding())
        .await
        .unwrap();
    assert_eq!(report.inline, 0);
    assert_eq!(server.requests().len(), 5);
    let params = request_body(server.requests().last().unwrap());
    assert_eq!(
        params["body"]
            .as_str()
            .unwrap()
            .matches(CONVERSATION_MARKER)
            .count(),
        1
    );
    assert!(
        server
            .requests()
            .last()
            .unwrap()
            .starts_with("POST /repos/owner/repo/issues/123/comments ")
    );
}

fn file_question(related_commits: &[&str], location_commit: &str) -> RenderInput {
    serde_json::from_value(json!({
        "ordered_commits": related_commits,
        "stages": [],
        "questions": [{
            "category": "rationale",
            "question": "Why?",
            "evidence": "Changed code",
            "why_it_matters": "Compatibility",
            "related_commits": related_commits,
            "location": {
                "commit": location_commit,
                "file": "src/main.rs"
            },
        }],
    }))
    .unwrap()
}

#[tokio::test]
async fn question_with_one_matching_related_commit_is_published_inline() {
    let input = file_question(&["abc1234"], "abc12345678");
    let mut replies = before_inline();
    replies.push(created());
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    let requests = server.requests();
    let request = requests.last().unwrap();
    assert!(request.starts_with("POST /repos/owner/repo/pulls/123/comments "));
    assert_eq!(request_body(request)["subject_type"], "file");
    assert_eq!(report.inline, 1);
    assert_eq!(report.urls.len(), 1);
}

#[tokio::test]
async fn question_with_multiple_related_commits_is_published_as_a_conversation_comment() {
    let input = file_question(&["abc1234", "def5678"], "abc1234");
    let server = Server::start(vec![
        pull(),
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
    assert!(
        server
            .requests()
            .last()
            .unwrap()
            .starts_with("POST /repos/owner/repo/issues/123/comments ")
    );
    assert_eq!(report.inline, 0);
    assert_eq!(report.urls.len(), 1);
}

#[tokio::test]
async fn question_with_a_mismatched_location_commit_is_published_as_a_conversation_comment() {
    let input = file_question(&["abc1234"], "def5678");
    let server = Server::start(vec![
        pull(),
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
    assert!(
        server
            .requests()
            .last()
            .unwrap()
            .starts_with("POST /repos/owner/repo/issues/123/comments ")
    );
    assert_eq!(report.inline, 0);
    assert_eq!(report.urls.len(), 1);
}

#[tokio::test]
async fn question_without_related_commits_is_published_as_a_conversation_comment() {
    let input = file_question(&[], "abc1234");
    let server = Server::start(vec![
        pull(),
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
    assert!(
        server
            .requests()
            .last()
            .unwrap()
            .starts_with("POST /repos/owner/repo/issues/123/comments ")
    );
    assert_eq!(report.inline, 0);
    assert_eq!(report.urls.len(), 1);
}

#[tokio::test]
async fn unlocated_questions_and_recommendations_share_one_comment() {
    let input = serde_json::from_value(json!({
        "ordered_commits": ["abc1234"],
        "stages": [],
        "questions": [{
            "category": "rationale",
            "question": "Why?",
            "evidence": "Evidence",
            "why_it_matters": "Reason",
            "related_commits": ["abc1234"]
        }],
        "recommendations": [{
            "kind": "split_commit",
            "message": "Split this",
            "rationale": "Separate concerns",
            "related_commits": ["abc1234"]
        }],
    }))
    .unwrap();
    let server = Server::start(vec![
        pull(),
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
    let body = request_body(server.requests().last().unwrap())["body"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(body.contains("## Review questions"));
    assert!(body.contains("## Structural recommendations"));
    assert_eq!(body.matches(CONVERSATION_MARKER).count(), 1);
    assert_eq!(crate::github::feedback::fingerprints(&body).len(), 2);
    assert_eq!(report.urls.len(), 1);
}

#[tokio::test]
async fn publishes_inline_on_a_changed_line() {
    let mut replies = before_inline();
    replies.push(created());
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &finding())
        .await
        .unwrap();
    let requests = server.requests();
    let request = requests.last().unwrap();
    assert!(request.starts_with("POST /repos/owner/repo/pulls/123/comments "));
    let params = request_body(request);
    assert_eq!(params["path"], "src/main.rs");
    assert_eq!(params["line"], 5);
    assert_eq!(params["side"], "RIGHT");
    assert!(params.get("subject_type").is_none());
    assert!(
        !params["body"]
            .as_str()
            .unwrap()
            .contains(CONVERSATION_MARKER)
    );
    assert_eq!(report.inline, 1);
}

#[tokio::test]
async fn unchanged_lines_fall_back_to_a_conversation_comment() {
    let mut input = finding();
    let RenderInput::Document(document) = &mut input;
    document.findings[0].location.as_mut().unwrap().line = Some(4);
    let mut replies = before_inline();
    replies.push(created());
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.inline, 0);
    assert_eq!(report.urls.len(), 1);
    let requests = server.requests();
    assert!(
        requests
            .last()
            .unwrap()
            .starts_with("POST /repos/owner/repo/issues/123/comments ")
    );
}

#[tokio::test]
async fn positions_from_later_file_pages_are_used() {
    let first_page = Reply::json(json!([]))
        .header("Link: <{base}repos/owner/repo/pulls/123/files?per_page=100&page=2>; rel=\"next\"");
    let mut replies = before_inline();
    replies.insert(3, first_page);
    replies.push(created());
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &file_finding())
        .await
        .unwrap();
    assert_eq!(report.inline, 1);
    assert_eq!(server.requests().len(), 7);
}

#[tokio::test]
async fn rerunning_after_partial_success_posts_only_the_missing_remainder() {
    let input = file_document();
    let mut replies = before_inline();
    let mut rejected = created();
    rejected.status = 403;
    replies.extend([created(), rejected]);
    let first = Server::start(replies).await;
    assert_matches!(
        first
            .client()
            .publish(&repository(), number(), &input)
            .await,
        Err(_)
    );
    let requests = first.requests();
    let inline_body = request_body(&requests[5])["body"]
        .as_str()
        .unwrap()
        .to_string();
    let failed_body = request_body(&requests[6])["body"]
        .as_str()
        .unwrap()
        .to_string();

    let second = Server::start(vec![
        pull(),
        Reply::json(json!([])),
        Reply::json(json!([{
            "body": inline_body.as_str()
        }])),
        created(),
    ])
    .await;
    let report = second
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.skipped, 1);
    assert_eq!(report.inline, 0);
    assert_eq!(
        request_body(second.requests().last().unwrap())["body"],
        failed_body
    );

    let third = Server::start(vec![
        pull(),
        Reply::json(json!([{
            "body": failed_body.as_str()
        }])),
        Reply::json(json!([{
            "body": inline_body.as_str()
        }])),
    ])
    .await;
    let report = third
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.skipped, 3);
    assert_eq!(third.requests().len(), 3);
}

#[tokio::test]
async fn zero_line_falls_back_to_a_conversation_comment() {
    let mut input = finding();
    let RenderInput::Document(document) = &mut input;
    document.findings[0].location.as_mut().unwrap().line = Some(0);
    let mut replies = before_inline();
    replies.push(created());
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.inline, 0);
    assert_eq!(report.urls.len(), 1);
    let requests = server.requests();
    assert!(
        requests
            .last()
            .unwrap()
            .starts_with("POST /repos/owner/repo/issues/123/comments ")
    );
}

#[tokio::test]
async fn line_outside_patch_falls_back_to_a_conversation_comment() {
    let mut input = finding();
    let RenderInput::Document(document) = &mut input;
    document.findings[0].location.as_mut().unwrap().line = Some(100);
    let mut replies = before_inline();
    replies.push(created());
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.inline, 0);
    assert_eq!(report.urls.len(), 1);
    let requests = server.requests();
    assert!(
        requests
            .last()
            .unwrap()
            .starts_with("POST /repos/owner/repo/issues/123/comments ")
    );
}
