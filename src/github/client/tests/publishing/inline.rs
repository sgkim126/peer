use super::*;

fn file_finding() -> RenderDocument {
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

fn file_document() -> RenderDocument {
    let mut input = document();
    for finding in &mut input.findings {
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
        pr_commits(),
        Reply::json(json!([])),
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
    let input = file_finding();
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
            .starts_with("Published 1 comment(s). 1 inline, 0 commit, 0 conversation.")
    );
    assert_eq!(requests.len(), 8);
}

#[tokio::test]
async fn separates_fallback_feedback_from_the_review_summary() {
    let mut replies = before_inline();
    let mut rejected = created();
    rejected.status = 422;
    replies.extend([rejected, created(), created(), created()]);
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &file_document())
        .await
        .unwrap();
    let requests = server.requests();
    assert!(requests[7].starts_with("POST /repos/owner/repo/pulls/123/comments "));
    assert!(requests[8].starts_with("POST /repos/owner/repo/commits/abc1234/comments "));
    assert!(requests[9].starts_with("POST /repos/owner/repo/commits/abc1234/comments "));
    let bodies = posted_bodies(&server);
    assert!(requests[10].starts_with("POST /repos/owner/repo/issues/123/comments "));
    assert!(bodies[1].contains("First issue"));
    assert!(!bodies[1].contains("Second issue"));
    assert!(bodies[2].contains("Second issue"));
    assert!(!bodies[2].contains("First issue"));
    for feedback in &bodies[1..3] {
        assert!(!feedback.contains("## Review summary"));
        assert!(!feedback.contains("<summary>Stage:"));
        assert!(!feedback.contains(CONVERSATION_MARKER));
        assert_eq!(crate::github::feedback::fingerprints(feedback).len(), 1);
    }
    let summary = &bodies[3];
    assert!(summary.contains("## Review summary"));
    assert!(summary.contains("<summary>Stage:"));
    assert!(summary.contains(CONVERSATION_MARKER));
    assert!(!summary.contains("First issue"));
    assert!(!summary.contains("Second issue"));
    assert_eq!(crate::github::feedback::fingerprints(summary).len(), 1);
    assert_eq!(report.inline, 0);
    assert_eq!(report.urls.len(), 3);
    assert_eq!(report.published, 3);
    assert!(
        report
            .to_string()
            .starts_with("Published 3 comment(s). 0 inline, 2 commit, 1 conversation.")
    );
}

#[tokio::test]
async fn keeps_summary_and_full_counts_even_when_every_item_is_inline() {
    let mut input = file_document();
    input.findings.pop();
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
            .starts_with("Published 2 comment(s). 1 inline, 0 commit, 1 conversation.")
    );
}

#[tokio::test]
async fn failure_to_load_files_falls_back_to_a_commit_comment() {
    let mut replies = before_publish();
    replies[5].status = 500;
    replies.push(created());
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &file_finding())
        .await
        .unwrap();
    assert_eq!(report.inline, 0);
    assert_eq!(server.requests().len(), 8);
    assert!(server.requests()[6].starts_with("GET /repos/owner/repo/pulls/123 "));
    let params = request_body(server.requests().last().unwrap());
    assert_eq!(
        params["body"]
            .as_str()
            .unwrap()
            .matches(CONVERSATION_MARKER)
            .count(),
        0
    );
    assert!(
        server
            .requests()
            .last()
            .unwrap()
            .starts_with("POST /repos/owner/repo/commits/abc1234/comments ")
    );
}

fn file_question(related_commits: &[&str], location_commit: &str) -> RenderDocument {
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
async fn question_with_multiple_related_commits_is_published_inline() {
    let mut input = file_question(&["abc1234", "def5678"], "abc1234");
    input.questions[0].location.as_mut().unwrap().file.line = Some(5);
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
    let params = request_body(request);
    assert_eq!(params["commit_id"], "abc1234");
    assert_eq!(params["path"], "src/main.rs");
    assert_eq!(params["line"], 5);
    assert_eq!(params["side"], "RIGHT");
    assert!(params.get("subject_type").is_none());
    let body = params["body"].as_str().unwrap();
    assert!(body.contains("**question/rationale**"));
    assert!(!body.contains(CONVERSATION_MARKER));
    assert_eq!(crate::github::feedback::fingerprints(body).len(), 1);
    assert_eq!(report.inline, 1);
    assert_eq!(report.published, 1);
    assert_eq!(report.urls.len(), 1);
}

#[tokio::test]
async fn question_with_a_mismatched_location_commit_is_published_inline() {
    let mut input = file_question(&["def5678"], "abc1234");
    input.questions[0].location.as_mut().unwrap().file.line = Some(5);
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
    let params = request_body(request);
    assert_eq!(params["commit_id"], "abc1234");
    assert_eq!(params["path"], "src/main.rs");
    assert_eq!(params["line"], 5);
    assert_eq!(params["side"], "RIGHT");
    assert!(params.get("subject_type").is_none());
    let body = params["body"].as_str().unwrap();
    assert!(body.contains("**question/rationale**"));
    assert!(!body.contains(CONVERSATION_MARKER));
    assert_eq!(crate::github::feedback::fingerprints(body).len(), 1);
    assert_eq!(report.inline, 1);
    assert_eq!(report.published, 1);
    assert_eq!(report.urls.len(), 1);
}

#[tokio::test]
async fn question_without_related_commits_is_published_inline() {
    let mut input = file_question(&[], "abc1234");
    input.questions[0].location.as_mut().unwrap().file.line = Some(5);
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
    let params = request_body(request);
    assert_eq!(params["commit_id"], "abc1234");
    assert_eq!(params["path"], "src/main.rs");
    assert_eq!(params["line"], 5);
    assert_eq!(params["side"], "RIGHT");
    assert!(params.get("subject_type").is_none());
    let body = params["body"].as_str().unwrap();
    assert!(body.contains("**question/rationale**"));
    assert!(!body.contains(CONVERSATION_MARKER));
    assert_eq!(crate::github::feedback::fingerprints(body).len(), 1);
    assert_eq!(report.inline, 1);
    assert_eq!(report.published, 1);
    assert_eq!(report.urls.len(), 1);
}

#[tokio::test]
async fn questions_outside_diff_lines_fall_back_to_file_comments() {
    for line in [3, 99] {
        let mut input = file_question(&["abc1234", "def5678"], "abc1234");
        input.questions[0].location.as_mut().unwrap().file.line = Some(line);
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
        let params = request_body(request);
        let body = params["body"].as_str().unwrap();
        assert!(body.contains("**question/rationale**"));
        assert!(!body.contains(CONVERSATION_MARKER));
        assert_eq!(params["subject_type"], "file");
        assert_eq!(report.inline, 1);
        assert_eq!(report.published, 1);
        assert_eq!(report.urls.len(), 1);
        assert_eq!(requests.len(), 8);
    }
}

#[tokio::test]
async fn unlocated_questions_and_recommendations_have_individual_commit_comments() {
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
        pr_commits(),
        Reply::json(json!([])),
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
    let requests = server.requests();
    let request = requests.last().unwrap();
    assert!(request.starts_with("POST /repos/owner/repo/commits/abc1234/comments "));
    let body = request_body(request)["body"].as_str().unwrap().to_string();
    assert!(posted_bodies(&server)[0].contains("**question/rationale**"));
    assert!(body.contains("**recommendation/split_commit**"));
    assert!(!body.contains("**question/rationale**"));
    assert_eq!(body.matches(CONVERSATION_MARKER).count(), 0);
    assert_eq!(crate::github::feedback::fingerprints(&body).len(), 1);
    assert_eq!(report.inline, 0);
    assert_eq!(report.commit_comments, 2);
    assert_eq!(report.published, 2);
    assert_eq!(report.urls.len(), 2);
    assert_eq!(requests.len(), 8);
    assert!(requests[5].starts_with("GET /repos/owner/repo/pulls/123 "));
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
async fn publishes_inline_on_a_context_line() {
    let mut input = finding();
    input.findings[0].location.as_mut().unwrap().line = Some(6);
    let mut replies = before_inline();
    replies.push(created());
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.inline, 1);
    assert_eq!(report.urls.len(), 1);
    let requests = server.requests();
    let request = requests.last().unwrap();
    assert!(request.starts_with("POST /repos/owner/repo/pulls/123/comments "));
    let params = request_body(request);
    assert_eq!(params["line"], 6);
    assert_eq!(params["side"], "RIGHT");
}

#[tokio::test]
async fn positions_from_later_file_pages_are_used() {
    let first_page = Reply::json(json!([]))
        .header("Link: <{base}repos/owner/repo/pulls/123/files?per_page=100&page=2>; rel=\"next\"");
    let mut replies = before_inline();
    replies.insert(5, first_page);
    replies.push(created());
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &file_finding())
        .await
        .unwrap();
    assert_eq!(report.inline, 1);
    assert_eq!(server.requests().len(), 9);
}

#[tokio::test]
async fn rerunning_after_partial_success_posts_only_the_missing_remainder() {
    let mut input = file_document();
    input.findings[1].commit = CommitHash::new("fedcba9").unwrap();
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
    let inline_body = request_body(&requests[7])["body"]
        .as_str()
        .unwrap()
        .to_string();
    let failed_body = request_body(&requests[8])["body"]
        .as_str()
        .unwrap()
        .to_string();

    let second = Server::start(vec![
        pull(),
        pr_commits(),
        Reply::json(json!([])),
        Reply::json(json!([{
            "body": inline_body.as_str()
        }])),
        Reply::json(json!([])),
        pull(),
        created(),
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
    assert_eq!(posted_bodies(&second)[0], failed_body);
    let summary_body = posted_bodies(&second)[1].clone();

    let third = Server::start(vec![
        pull(),
        pr_commits(),
        Reply::json(json!([{
            "body": failed_body.as_str()
        }, {"body": summary_body}])),
        Reply::json(json!([{
            "body": inline_body.as_str()
        }])),
        Reply::json(json!([])),
    ])
    .await;
    let report = third
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.skipped, 3);
    assert_eq!(third.requests().len(), 5);
}

#[tokio::test]
async fn zero_line_falls_back_to_a_file_comment() {
    let mut input = finding();
    input.findings[0].location.as_mut().unwrap().line = Some(0);
    let mut replies = before_inline();
    replies.push(created());
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.inline, 1);
    assert_eq!(report.urls.len(), 1);
    let requests = server.requests();
    assert_eq!(
        request_body(requests.last().unwrap())["subject_type"],
        "file"
    );
    assert!(
        requests
            .last()
            .unwrap()
            .starts_with("POST /repos/owner/repo/pulls/123/comments ")
    );
}

#[tokio::test]
async fn line_outside_patch_falls_back_to_a_file_comment() {
    let mut input = finding();
    input.findings[0].location.as_mut().unwrap().line = Some(100);
    let mut replies = before_inline();
    replies.push(created());
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.inline, 1);
    assert_eq!(report.urls.len(), 1);
    let requests = server.requests();
    assert_eq!(
        request_body(requests.last().unwrap())["subject_type"],
        "file"
    );
    assert!(
        requests
            .last()
            .unwrap()
            .starts_with("POST /repos/owner/repo/pulls/123/comments ")
    );
}
